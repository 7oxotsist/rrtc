// src/main.rs
use anyhow::{anyhow, Result};
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
use str0m::{Candidate, Event, Input, Output, Rtc, RtcError};
use str0m::media::MediaData;  // Изменено: правильный импорт MediaData

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
    ws_send: tokio::sync::mpsc::UnboundedSender<Message>, // Изменено: используем канал
    participant_id: String,
    name: String,
    muted: bool,
    video_on: bool,
    screen_sharing: bool,
}

struct Room {
    peers: HashMap<String, Peer>,
    addr_to_participant: HashMap<SocketAddr, String>,
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
                    let contents = buf[..len].to_vec().into(); // Исправлено: берем срез, а не клонируем весь буфер
                    
                    let mut rooms_guard = rooms_udp.lock().await;
                    if let Some((room_id, participant_id)) = find_peer_by_addr(&rooms_guard, src) {
                        if let Some(room) = rooms_guard.get_mut(&room_id) {
                            if let Some(peer) = room.peers.get_mut(&participant_id) {
                                let input = Input::Receive(now, str0m::net::Receive {
                                    source: src,
                                    destination: udp_clone.local_addr().unwrap(),
                                    contents,
                                    proto: str0m::net::Protocol::Udp,
                                });
                                if let Err(e) = peer.rtc.handle_input(input) {
                                    error!("handle_input error: {}", e);
                                }
                                // Нужно получить ws_send из peer, но она в Mutex - временно уберем
                                // drive_rtc(&mut peer.rtc, &mut peer.ws_send, &rooms_guard, room_id, participant_id).await;
                            }
                        }
                    }
                }
                Err(e) => error!("UDP recv error: {}", e),
            }
        }
    });

    loop {
        let (stream, addr) = signaling_listener.accept().await?;
        info!("New WS connection from {}", addr);
        let rooms_clone = rooms.clone();
        let udp_clone = udp.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_ws_connection(stream, rooms_clone, udp_clone, addr).await {
                error!("WS handler error: {}", e);
            }
        });
    }
}

async fn handle_ws_connection(
    stream: tokio::net::TcpStream,
    rooms: Rooms,
    udp: Arc<UdpSocket>,
    client_addr: SocketAddr,
) -> Result<()> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (ws_send, mut ws_recv) = ws_stream.split();

    // Создаем канал для отправки сообщений WebSocket
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ws_sender = ws_send;
    
    // Запускаем задачу для отправки сообщений через WebSocket
    let ws_send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = ws_sender.send(msg).await {
                error!("Failed to send WS message: {}", e);
                break;
            }
        }
    });

    // Первый message — join
    let msg = ws_recv.next().await.ok_or(anyhow!("no join message"))??;
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

    // Запоминаем адрес клиента
    room.addr_to_participant.insert(client_addr, participant_id.clone());

    let peer = Peer {
        rtc,
        ws_send: tx.clone(),
        participant_id: participant_id.clone(),
        name: name.clone(),
        muted: true,
        video_on: true,
        screen_sharing: false,
    };

    room.peers.insert(participant_id.clone(), peer);

    // Отправляем Joined сообщение
    tx.send(Message::text(
        serde_json::to_string(&ServerMessage::Joined {
            your_id: participant_id.clone(),
        })?
    ))?;

    // Отправляем broadcast о новом участнике другим клиентам
    for (id, other_peer) in &room.peers {
        if id != &participant_id {
            let _ = other_peer.ws_send.send(Message::text(
                serde_json::to_string(&ServerMessage::ParticipantJoined {
                    id: participant_id.clone(),
                    name: name.clone(),
                })?
            ));
        }
    }

    drop(rooms_guard);

    // Основной цикл обработки WS сообщений
    while let Some(Ok(msg)) = ws_recv.next().await {
        if let Message::Text(text) = msg {
            if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                if let Err(e) = handle_client_message(&rooms, &tx, room_id.clone(), &participant_id, client_msg).await {
                    error!("Error handling client message: {}", e);
                }
            }
        }
    }

    // Cleanup
    cleanup_peer(&rooms, room_id, &participant_id).await;
    ws_send_task.abort();

    Ok(())
}

async fn handle_client_message(
    rooms: &Rooms,
    tx: &tokio::sync::mpsc::UnboundedSender<Message>,
    room_id: String,
    participant_id: &str,
    msg: ClientMessage,
) -> Result<()> {
    let mut rooms_guard = rooms.lock().await;
    let room = rooms_guard.get_mut(&room_id).ok_or(anyhow!("no room"))?;
    let peer = room.peers.get_mut(participant_id).ok_or(anyhow!("no peer"))?;

    match msg {
        ClientMessage::Offer { sdp } => {
            let offer = sdp.parse()?;
            let answer = peer.rtc.sdp_api().accept_offer(offer)?;
            tx.send(Message::text(json!({
                "type": "answer",
                "sdp": answer.to_string()
            }).to_string()))?;
        }
        ClientMessage::Candidate { candidate } => {
            let cand = Candidate::from_sdp_string(&candidate)?;
            peer.rtc.add_remote_candidate(cand);
        }
        ClientMessage::StateUpdate { muted, video_on, screen_sharing } => {
            peer.muted = muted;
            peer.video_on = video_on;
            peer.screen_sharing = screen_sharing;

            // Отправляем обновление состояния всем
            let msg = ServerMessage::StateUpdate {
                participant_id: participant_id.to_string(),
                muted,
                video_on,
                screen_sharing,
            };
            let text = serde_json::to_string(&msg)?;
            
            for (id, other_peer) in &room.peers {
                if id != participant_id {
                    let _ = other_peer.ws_send.send(Message::text(text.clone()));
                }
            }
        }
        _ => {}
    }

    // Drive RTC events
    drive_rtc(&mut peer.rtc, tx, &rooms_guard, &room_id, participant_id).await;

    Ok(())
}

async fn drive_rtc(
    rtc: &mut Rtc,
    tx: &tokio::sync::mpsc::UnboundedSender<Message>,
    rooms: &HashMap<String, Room>,
    room_id: &str,
    participant_id: &str,
) {
    loop {
        match rtc.poll_output() {
            Ok(Output::Timeout(_)) => break,
            Ok(Output::Transmit(tx_data)) => {
                // Здесь нужно отправить данные через UDP
                // Для этого нужен доступ к UDP сокету
                // Пока пропускаем
            }
            Ok(Output::Event(ev)) => {
                match ev {
                    Event::IceCandidate(cand) => { // Исправлено: IceCandidate вместо Candidate
                        let _ = tx.send(Message::text(json!({
                            "type": "candidate",
                            "candidate": cand.to_sdp_string()
                        }).to_string()));
                    }
                    Event::Media(md) => { // Исправлено: правильный паттерн для MediaData
                        if let str0m::media::MediaEvent::Data(md) = md {
                            forward_media_data(rooms, room_id, participant_id, md).await;
                        }
                    }
                    _ => {}
                }
            }
            Err(_) => break,
        }
    }
}

async fn forward_media_data(
    rooms: &HashMap<String, Room>,
    room_id: &str,
    from_id: &str,
    md: MediaData,
) {
    if let Some(room) = rooms.get(room_id) {
        for (to_id, to_peer) in &room.peers {
            if to_id == from_id {
                continue;
            }

            // Проверяем настройки получателя
            if md.kind == str0m::media::MediaKind::Audio && to_peer.muted {
                continue;
            }
            if md.kind == str0m::media::MediaKind::Video && !to_peer.video_on {
                continue;
            }

            // Пытаемся получить writer и отправить данные
            if let Ok(writer) = to_peer.rtc.writer(md.mid).ok_or(err) {
                let _ = writer.write(md.pt, md.wallclock, md.rtp_time, &md.data);
            }
        }
    }
}

async fn cleanup_peer(rooms: &Rooms, room_id: String, participant_id: &str) {
    let mut rooms_guard = rooms.lock().await;
    if let Some(room) = rooms_guard.get_mut(&room_id) {
        room.peers.remove(participant_id);
        room.addr_to_participant.retain(|_, id| id != participant_id);
        
        // Можно отправить сообщение о выходе участника
        // broadcast_to_room(room, ServerMessage::ParticipantLeft { id: participant_id.to_string() }, None).await;
    }
}

fn find_peer_by_addr(rooms: &HashMap<String, Room>, src: SocketAddr) -> Option<(String, String)> {
    for (room_id, room) in rooms {
        if let Some(participant_id) = room.addr_to_participant.get(&src) {
            return Some((room_id.clone(), participant_id.clone()));
        }
    }
    None
}