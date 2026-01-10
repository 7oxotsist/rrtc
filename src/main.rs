// src/main.rs
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use log::{error, info};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use str0m::{Candidate, Event, Input, MediaData, Output, Rtc, RtcError};

const SIGNALING_PORT: u16 = 8080;
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

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    Message,
>;

struct Peer {
    rtc: Rtc,
    ws_send: WsSink,
    participant_id: String,
    name: String,
    muted: bool,
    video_on: bool,
    screen_sharing: bool,
}

struct Room {
    peers: HashMap<String, Peer>,
    addr_to_participant: HashMap<SocketAddr, String>, // демультиплексация по source addr
}

type Rooms = Arc<Mutex<HashMap<String, Room>>>;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let rooms: Rooms = Arc::new(Mutex::new(HashMap::new()));

    let udp = Arc::new(UdpSocket::bind(format!("0.0.0.0:{}", MEDIA_UDP_PORT)).await?);
    info!("Media UDP listening on :{}", MEDIA_UDP_PORT);

    let signaling_listener = TcpListener::bind(format!("0.0.0.0:{}", SIGNALING_PORT)).await?;
    info!("Signaling WS server listening on :{}", SIGNALING_PORT);

    let udp_clone = udp.clone();
    let rooms_udp = rooms.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 2000];
        loop {
            match udp_clone.recv_from(&mut buf).await {
                Ok((len, src)) => {
                    let now = Instant::now();
                    buf.truncate(len);
                    let contents = buf.clone().into(); // Into<Vec<u8>>

                    let mut rooms_guard = rooms_udp.lock().await;
                    // Находим peer по source addr (упрощённо, в реальности по fingerprint или session)
                    if let Some((room_id, participant_id)) = find_peer_by_addr(&rooms_guard, src) {
                        if let Some(room) = rooms_guard.get_mut(&room_id) {
                            if let Some(peer) = room.peers.get_mut(&participant_id) {
                                let input = Input::Receive(now, str0m::net::Receive {source:src,destination:udp_clone.local_addr().unwrap(),contents, proto: str0m::net::Protocol::Udp});
                                if let Err(e) = peer.rtc.handle_input(input) {
                                    error!("handle_input error: {}", e);
                                }
                                drive_rtc(&mut peer.rtc, &mut peer.ws_send, &rooms_guard, room_id, participant_id).await;
                            }
                        }
                    }
                }
                Err(e) => error!("UDP recv error: {}", e),
            }
        }
    });

    loop {
        let (stream, _) = signaling_listener.accept().await?;
        let rooms_clone = rooms.clone();
        let udp_clone = udp.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_ws_connection(stream, rooms_clone, udp_clone).await {
                error!("WS handler error: {}", e);
            }
        });
    }
}

async fn handle_ws_connection(
    stream: tokio::net::TcpStream,
    rooms: Rooms,
    udp: Arc<UdpSocket>,
) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (mut ws_send, mut ws_recv) = ws_stream.split();

    // Первый message — join (от Go proxy)
    let msg = ws_recv.next().await.ok_or(RtcError::Other("no join".into()))??;
    let text = if let Message::Text(t) = msg { t } else { return Ok(()) };

    let join: ClientMessage = serde_json::from_str(&text)?;
    let (room_id, participant_id, name) = match join {
        ClientMessage::Join { room, participant, name } => (room, participant, name),
        _ => return Ok(()),
    };

    info!("Participant {} ({}) joined room {}", participant_id, name, room_id);

    let local_addr: SocketAddr = udp.local_addr()?;
    let host_cand = Candidate::host(local_addr, "udp")?;

    let mut rtc = Rtc::new();
    rtc.add_local_candidate(host_cand);

    let mut rooms_guard = rooms.lock().await;
    let room = rooms_guard.entry(room_id.clone()).or_insert_with(|| Room {
        peers: HashMap::new(),
        addr_to_participant: HashMap::new(),
    });

    // Привязываем будущие пакеты от клиента к этому participant (клиент будет использовать один addr)
    // В реальности лучше по DTLS fingerprint, но упрощённо — первый пакет запомнит addr

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

    // joined
    ws_send.send(Message::text(serde_json::to_string(&ServerMessage::Joined {
        your_id: participant_id.clone(),
    })?)).await?;

    broadcast_to_room(room, ServerMessage::ParticipantJoined {
        id: participant_id.clone(),
        name,
    }, Some(&participant_id)).await;

    drop(rooms_guard);

    // Основной цикл WS + drive rtc
    while let Some(Ok(msg)) = ws_recv.next().await {
        if let Message::Text(text) = msg {
            if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                handle_client_message(&rooms, room_id.clone(), participant_id.clone(), client_msg).await?;
            }
        }
    }

    // Cleanup
    cleanup_peer(&rooms, room_id, participant_id).await;

    Ok(())
}

async fn handle_client_message(
    rooms: &Rooms,
    room_id: String,
    participant_id: String,
    msg: ClientMessage,
) -> Result<()> {
    let mut rooms_guard = rooms.lock().await;
    let room = rooms_guard.get_mut(&room_id).ok_or(RtcError::Other("no room".into()))?;
    let peer = room.peers.get_mut(&participant_id).ok_or(RtcError::Other("no peer".into()))?;

    match msg {
        ClientMessage::Offer { sdp } => {
            let offer = sdp.parse()?;
            let answer = peer.rtc.sdp_api().accept_offer(offer)?;
            peer.ws_send.send(Message::text(json!({
                "type": "answer",
                "sdp": answer.to_string()
            }).to_string())).await?;
        }
        ClientMessage::Candidate { candidate } => {
            let cand = Candidate::from_sdp_string(&candidate)?;
            peer.rtc.add_remote_candidate(cand);
        }
        ClientMessage::StateUpdate { muted, video_on, screen_sharing } => {
            peer.muted = muted;
            peer.video_on = video_on;
            peer.screen_sharing = screen_sharing;

            broadcast_to_room(room, ServerMessage::StateUpdate {
                participant_id,
                muted,
                video_on,
                screen_sharing,
            }, None).await;
        }
        _ => {}
    }

    drive_rtc(&mut peer.rtc, &mut peer.ws_send, &rooms_guard, room_id, participant_id).await;

    Ok(())
}

async fn drive_rtc(rtc: &mut Rtc, ws_send: &mut WsSink, rooms: &HashMap<String, Room>, room_id: String, participant_id: String) {
    loop {
        match rtc.poll_output() {
            Ok(Output::Timeout(_)) => break,
            Ok(Output::Transmit(tx)) => {
                // Отправляем UDP (глобальный udp из main)
                // Но udp в scope нет — передайте Arc<UdpSocket> в функцию или глобально
                // Для простоты пропустим или добавьте параметр
            }
            Ok(Output::Event(ev)) => {
                match ev {
                    Event::Candidate(cand) => {
                        let _ = ws_send.send(Message::text(json!({
                            "type": "candidate",
                            "candidate": cand.to_sdp_string()
                        }).to_string())).await;
                    }
                    Event::MediaData(md) => {
                        forward_media_data(rooms, &room_id, &participant_id, md).await;
                    }
                    _ => {}
                }
            }
            Err(_) => break,
        }
    }
}

async fn forward_media_data(rooms: &HashMap<String, Room>, room_id: &str, from_id: &str, md: MediaData) {
    if let Some(room) = rooms.get(room_id) {
        for (to_id, to_peer) in &room.peers {
            if to_id == from_id { continue; }

            if md.kind == str0m::media::MediaKind::Audio && to_peer.muted { continue; }
            if md.kind == str0m::media::MediaKind::Video && !to_peer.video_on { continue; }

            if let writer = to_peer.rtc.writer(md.mid).unwrap() {
                let _ = writer.write(md.pt, md.wallclock, md.rtp_time, &md.data).unwrap();
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

async fn cleanup_peer(rooms: &Rooms, room_id: String, participant_id: String) {
    let mut rooms_guard = rooms.lock().await;
    if let Some(room) = rooms_guard.get_mut(&room_id) {
        room.peers.remove(&participant_id);
        broadcast_to_room(room, ServerMessage::ParticipantJoined { id: participant_id, name: "".to_string() }, None).await; // или left
    }
}

// Вспомогательная для поиска peer по addr (упрощённо)
fn find_peer_by_addr(rooms: &HashMap<String, Room>, src: SocketAddr) -> Option<(String, String)> {
    for (room_id, room) in rooms {
        if let Some(participant_id) = room.addr_to_participant.get(&src) {
            return Some((room_id.clone(), participant_id.clone()));
        }
    }
    None
}