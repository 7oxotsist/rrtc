use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use str0m::media::{MediaKind, TrackId};
use str0m::net::{NetConfig, Receive, Send};
use str0m::{Candidate, Change, Event, Input, Output, Rtc, RtcConfig};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::protocol::Message;
use url::Url;

const SIGNALING_PORT: u16 = 8081;
const MEDIA_UDP_PORT: u16 = 5000;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "join")]
    Join { room: String, participant: String, name: String },
    #[serde(rename = "offer")]
    Offer { sdp: String },
    #[serde(rename = "candidate")]
    Candidate { candidate: String },
    #[serde(rename = "state_update")]
    StateUpdate { muted: bool, video_on: bool, screen_sharing: bool },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum ServerMessage {
    #[serde(rename = "joined")]
    Joined { your_id: String },
    #[serde(rename = "answer")]
    Answer { sdp: String },
    #[serde(rename = "candidate")]
    Candidate { candidate: String },
    #[serde(rename = "participant_joined")]
    ParticipantJoined { id: String, name: String },
    #[serde(rename = "state_update")]
    StateUpdate { participant_id: String, muted: bool, video_on: bool, screen_sharing: bool },
}

struct Peer {
    rtc: Rtc,
    ws_send: futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Message>,
    participant_id: String,
    name: String,
    muted: bool,
    video_on: bool,
    screen_sharing: bool,
}

struct Room {
    peers: HashMap<String, Peer>, // participant_id -> Peer
}

type Rooms = Arc<Mutex<HashMap<String, Room>>>; // room_id -> Room

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));

    // Один UDP-сокет для всего медиатрафика
    let udp = UdpSocket::bind(format!("0.0.0.0:{}", MEDIA_UDP_PORT)).await?;
    let net = NetConfig::new(udp);

    let signaling_listener = TcpListener::bind(format!("0.0.0.0:{}", SIGNALING_PORT)).await?;
    info!("Signaling WS server listening on :{}", SIGNALING_PORT);
    info!("Media UDP on :{}", MEDIA_UDP_PORT);

    loop {
        tokio::select! {
            // Новое WS-подключение (от Go-прокси)
            Ok((stream, _)) = signaling_listener.accept() => {
                let rooms_clone = rooms.clone();
                let net_clone = net.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_ws_connection(stream, rooms_clone, net_clone).await {
                        error!("WS handler error: {}", e);
                    }
                });
            }

            // Обработка сетевого трафика str0m (receive/send)
            Ok((peer_id, output)) = net_clone.recv() => {
                if let Err(e) = handle_str0m_output(rooms.clone(), peer_id, output).await {
                    error!("str0m output error: {}", e);
                }
            }
        }
    }
}

async fn handle_ws_connection(
    stream: tokio::net::TcpStream,
    rooms: Rooms,
    net: NetConfig,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut ws_send, mut ws_recv) = ws.split();

    // Читаем query параметры из handshake (Go передаёт ?room=...&participant=...)
    // tokio-tungstenite не даёт прямой доступ, но можно из первого сообщения или предположить join первым
    let first_msg = match ws_recv.next().await {
        Some(Ok(Message::Text(text))) => text,
        _ => return Ok(()),
    };

    let join: ClientMessage = serde_json::from_str(&first_msg)?;
    let (room_id, participant_id, name) = match join {
        ClientMessage::Join { room, participant, name } => (room, participant, name),
        _ => return Ok(()),
    };

    info!("Client joined room {} as participant {} ({})", room_id, participant_id, name);

    // Создаём или получаем комнату
    let mut rooms_guard = rooms.lock().await;
    let room = rooms_guard.entry(room_id.clone()).or_insert_with(|| Room { peers: HashMap::new() });

    // Создаём str0m Rtc для этого пира
    let mut rtc = Rtc::builder()
        .set_net_config(net.clone())
        .build();

    // Добавляем локальные ICE-кандидаты (host)
    rtc.add_local_candidate(Candidate::host(SocketAddr::new("0.0.0.0".parse()?, MEDIA_UDP_PORT), "udp")?);

    // STUN для public если нужно (добавьте later)

    let peer = Peer {
        rtc,
        ws_send,
        participant_id: participant_id.clone(),
        name: name.clone(),
        muted: true,
        video_on: true,
        screen_sharing: false,
    };

    room.peers.insert(participant_id.clone(), peer);

    // Отправляем joined
    ws_send.send(Message::text(serde_json::to_string(&ServerMessage::Joined {
        your_id: participant_id.clone(),
    })?)).await?;

    
    broadcast_to_room(&room, ServerMessage::ParticipantJoined {
        id: participant_id.clone(),
        name,
    }, None).await;

    drop(rooms_guard); // unlock

    // Основной цикл обработки сообщений от клиента и str0m событий
    loop {
        tokio::select! {
            // Сообщение от WS (offer, candidate, state_update)
            Some(Ok(msg)) = ws_recv.next() => {
                if let Message::Text(text) = msg {
                    if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                        handle_client_message(&rooms, room_id.clone(), participant_id.clone(), client_msg).await?;
                    }
                } else if msg.is_close() {
                    break;
                }
            }

            // События от str0m (ICE, media, changes)
            event = room.peers.get_mut(&participant_id).unwrap().rtc.poll_event() => {
                match event {
                    Some(Event::IceCandidate(candidate)) => {
                        // Отправляем candidate клиенту
                        let _ = room.peers.get_mut(&participant_id).unwrap().ws_send.send(Message::text(json!({
                            "type": "candidate",
                            "candidate": candidate.to_sdp_string()
                        }).to_string())).await;
                    }
                    Some(Event::Media(media)) => {
                        // Получили медиа от этого пира — форвардим всем остальным в комнате
                        forward_media(&rooms, room_id.clone(), participant_id.clone(), media).await;
                    }
                    Some(Event::Change(change)) => {
                        if change.remote_sdp.is_some() {
                            // После accept offer может быть answer
                            if let Some(answer) = change.local_sdp {
                                let _ = room.peers.get_mut(&participant_id).unwrap().ws_send.send(Message::text(json!({
                                    "type": "answer",
                                    "sdp": answer
                                }).to_string())).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Cleanup при disconnect
    let mut rooms_guard = rooms.lock().await;
    if let Some(room) = rooms_guard.get_mut(&room_id) {
        room.peers.remove(&participant_id);
        broadcast_to_room(room, ServerMessage::ParticipantJoined { id: participant_id, name: "".to_string() } , None).await; // или participant_left
    }

    Ok(())
}

async fn handle_client_message(
    rooms: &Rooms,
    room_id: String,
    participant_id: String,
    msg: ClientMessage,
) -> Result<()> {
    let mut rooms_guard = rooms.lock().await;
    let room = rooms_guard.get_mut(&room_id).unwrap();
    let peer = room.peers.get_mut(&participant_id).unwrap();

    match msg {
        ClientMessage::Offer { sdp } => {
            // Принимаем offer
            peer.rtc.accept_offer(sdp.parse()?)?;
        }
        ClientMessage::Candidate { candidate } => {
            peer.rtc.add_remote_candidate(candidate.parse()?)?;
        }
        ClientMessage::StateUpdate { muted, video_on, screen_sharing } => {
            peer.muted = muted;
            peer.video_on = video_on;
            peer.screen_sharing = screen_sharing;
            // Broadcast state (Go тоже может, но дублируем)
            broadcast_to_room(room, ServerMessage::StateUpdate {
                participant_id,
                muted,
                video_on,
                screen_sharing,
            }, Some(&participant_id)).await;
        }
        _ => {}
    }

    Ok(())
}

async fn forward_media(rooms: &Rooms, room_id: String, from_id: String, media: Receive) {
    let rooms_guard = rooms.lock().await;
    if let Some(room) = rooms_guard.get(&room_id) {
        for (to_id, to_peer) in &room.peers {
            if to_id == &from_id { continue; }

            if media.kind == MediaKind::Audio && to_peer.muted { continue; }
            if media.kind == MediaKind::Video && !to_peer.video_on { continue; }

            // Форвардим трек
            if let Some(send) = to_peer.rtc.direct_api().send(media.payload) {
                // str0m handles send
            }
        }
    }
}

async fn broadcast_to_room(room: &Room, msg: ServerMessage, exclude_id: Option<&String>) {
    let text = serde_json::to_string(&msg).unwrap();
    for (id, peer) in &room.peers {
        if exclude_id.map_or(false, |ex| ex == id) { continue; }
        let _ = peer.ws_send.send(Message::text(text.clone())).await;
    }
}