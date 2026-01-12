use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tokio_tungstenite::tungstenite::Message;

mod config;
mod messages;
mod peer;
mod room;

use config::ServerConfig;
use messages::{ClientMessage, ParticipantInfo, ServerMessage};
use peer::{Peer, PeerBuilder};
use room::RoomManager;

#[tokio::main]
async fn main() -> Result<()> {
    // Инициализация логирования
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    info!("Starting Rust WebRTC SFU Server");

    // Загрузка конфигурации
    let config = ServerConfig::load()?;
    config.validate()?;

    info!("Configuration loaded:");
    info!("  Listen address: {}", config.listen_address);
    info!("  Signaling port: {}", config.signaling_port);
    info!("  ICE servers: {}", config.ice_servers.len());
    info!("  Max participants per room: {}", config.max_participants_per_room);

    let config = Arc::new(config);

    // Создание менеджера комнат
    let room_manager = Arc::new(RoomManager::new());

    // Запуск фоновой задачи для очистки пустых комнат
    let rm_cleanup = room_manager.clone();
    let cleanup_interval = config.cleanup_interval_secs;
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(cleanup_interval));
        loop {
            interval.tick().await;
            rm_cleanup.cleanup_all_empty_rooms().await;
        }
    });

    // Запуск WebSocket сервера
    let addr = format!("{}:{}", config.listen_address, config.signaling_port);
    let listener = TcpListener::bind(&addr).await?;

    info!("WebRTC SFU listening on {}", addr);
    info!("Server is ready to accept connections");

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        info!("New connection from {}", peer_addr);

        let room_manager = room_manager.clone();
        let config = config.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, room_manager, config).await {
                error!("Connection error from {}: {}", peer_addr, e);
            }
        });
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    room_manager: Arc<RoomManager>,
    config: Arc<ServerConfig>,
) -> Result<()> {
    // Принимаем WebSocket соединение
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // Создаем канал для отправки сообщений клиенту
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Задача для отправки сообщений в WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = ws_sink.send(msg).await {
                error!("Failed to send WebSocket message: {}", e);
                break;
            }
        }
    });

    // Ожидаем сообщение Join
    let msg = match ws_stream.next().await {
        Some(Ok(Message::Text(text))) => text,
        Some(Ok(Message::Close(_))) => {
            info!("Client closed connection before joining");
            send_task.abort();
            return Ok(());
        }
        _ => {
            warn!("Invalid first message from client");
            send_task.abort();
            return Ok(());
        }
    };

    let join_msg: ClientMessage = match serde_json::from_str(&msg) {
        Ok(msg) => msg,
        Err(e) => {
            error!("Failed to parse join message: {}", e);
            send_task.abort();
            return Ok(());
        }
    };

    let (room_id, participant_id, name) = match join_msg {
        ClientMessage::Join {
            room,
            participant,
            name,
        } => (room, participant, name),
        _ => {
            error!("Expected join message");
            send_task.abort();
            return Ok(());
        }
    };

    info!(
        "Participant {} ({}) joining room {}",
        participant_id, name, room_id
    );

    // Получаем или создаем комнату
    let room = room_manager.get_or_create_room(room_id.clone()).await;

    // Проверяем лимит участников
    if room.peer_count().await >= config.max_participants_per_room {
        error!("Room {} is full", room_id);
        let _ = tx.send(Message::text(
            serde_json::to_string(&ServerMessage::Error {
                message: "Room is full".to_string(),
                code: Some(403),
            })
            .unwrap_or_default(),
        ));
        send_task.abort();
        return Ok(());
    }

    // Создаем Peer с ICE серверами из конфигурации
    let ice_servers = config.get_rtc_ice_servers();
    let peer = match PeerBuilder::new(participant_id.clone(), name.clone(), tx.clone())
        .with_ice_servers(ice_servers)
        .build()
        .await
    {
        Ok(peer) => Arc::new(peer),
        Err(e) => {
            error!("Failed to create peer: {}", e);
            send_task.abort();
            return Err(e);
        }
    };

    // Настраиваем обработчик входящих треков
    let room_clone = room.clone();
    let peer_id_clone = participant_id.clone();

    peer.pc
        .on_track(Box::new(move |track, _receiver, _transceiver| {
            let room = room_clone.clone();
            let from_id = peer_id_clone.clone();
            let track = track.clone();

            tokio::spawn(async move {
                if let Err(e) = room.handle_incoming_track(from_id, track).await {
                    error!("Error handling track: {}", e);
                }
            });

            Box::pin(async {})
        }));

    // Получаем список существующих участников
    let existing_peers = room.get_all_peers().await;
    let mut participants_info = Vec::new();

    for existing_peer in &existing_peers {
        if existing_peer.id != participant_id {
            let (muted, video_on, screen_sharing) = existing_peer.get_state().await;
            participants_info.push(ParticipantInfo::with_state(
                existing_peer.id.clone(),
                existing_peer.name.clone(),
                muted,
                video_on,
                screen_sharing,
            ));
        }
    }

    // Добавляем участника в комнату
    room.add_peer(peer.clone()).await?;

    // Отправляем подтверждение присоединения
    peer.send_message(ServerMessage::Joined {
        your_id: participant_id.clone(),
        participants: participants_info,
    })?;

    info!(
        "Peer {} successfully joined room {} ({} participants)",
        participant_id,
        room_id,
        room.peer_count().await
    );

    // Обрабатываем входящие сообщения от клиента
    let peer_for_loop = peer.clone();
    let room_for_loop = room.clone();

    while let Some(msg_result) = ws_stream.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(client_msg) => {
                        if let Err(e) =
                            handle_client_message(client_msg, peer_for_loop.clone(), room_for_loop.clone())
                                .await
                        {
                            error!("Error handling message: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse client message: {}", e);
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Client {} closed connection", participant_id);
                break;
            }
            Ok(Message::Ping(data)) => {
                let _ = peer_for_loop.ws_tx.send(Message::Pong(data));
            }
            Ok(Message::Pong(_)) => {
                // Pong received
            }
            Err(e) => {
                error!("WebSocket error for {}: {}", participant_id, e);
                break;
            }
            _ => {}
        }
    }

    // Очистка при отключении
    info!("Peer {} disconnecting from room {}", participant_id, room_id);
    room.remove_peer(&participant_id).await?;

    // Очищаем комнату если она пуста
    room_manager.cleanup_empty_room(&room_id).await;

    send_task.abort();

    Ok(())
}

async fn handle_client_message(
    msg: ClientMessage,
    peer: Arc<Peer>,
    room: Arc<room::Room>,
) -> Result<()> {
    match msg {
        ClientMessage::Offer { sdp } => {
            info!("Received offer from peer {}", peer.id);
            let answer_sdp = peer.handle_offer(sdp).await?;
            peer.send_message(ServerMessage::Answer { sdp: answer_sdp })?;
        }

        ClientMessage::Answer { sdp } => {
            info!("Received answer from peer {} (unexpected)", peer.id);
            // В SFU модели клиент обычно не отправляет answer, но обрабатываем на всякий случай
            let answer = webrtc::peer_connection::sdp::session_description::RTCSessionDescription::answer(sdp)?;
            peer.pc.set_remote_description(answer).await?;
        }

        ClientMessage::Candidate { candidate } => {
            peer.add_ice_candidate(candidate).await?;
        }

        ClientMessage::StateUpdate {
            muted,
            video_on,
            screen_sharing,
        } => {
            peer.update_state(muted, video_on, screen_sharing).await;

            // Транслируем обновление состояния другим участникам
            room.broadcast_message(
                &peer.id,
                ServerMessage::StateUpdate {
                    participant_id: peer.id.clone(),
                    muted,
                    video_on,
                    screen_sharing,
                },
            )
            .await;
        }

        ClientMessage::StartScreenShare => {
            info!("Peer {} started screen sharing", peer.id);
            peer.update_state(*peer.muted.read().await, *peer.video_on.read().await, true)
                .await;

            room.broadcast_message(
                &peer.id,
                ServerMessage::ScreenShareStarted {
                    participant_id: peer.id.clone(),
                },
            )
            .await;
        }

        ClientMessage::StopScreenShare => {
            info!("Peer {} stopped screen sharing", peer.id);
            peer.update_state(*peer.muted.read().await, *peer.video_on.read().await, false)
                .await;

            room.broadcast_message(
                &peer.id,
                ServerMessage::ScreenShareStopped {
                    participant_id: peer.id.clone(),
                },
            )
            .await;
        }

        ClientMessage::Ping => {
            peer.send_message(ServerMessage::Pong)?;
        }

        ClientMessage::GetParticipants => {
            let peers = room.get_all_peers().await;
            let mut participants = Vec::new();

            for p in peers {
                if p.id != peer.id {
                    let (muted, video_on, screen_sharing) = p.get_state().await;
                    participants.push(ParticipantInfo::with_state(
                        p.id.clone(),
                        p.name.clone(),
                        muted,
                        video_on,
                        screen_sharing,
                    ));
                }
            }

            peer.send_message(ServerMessage::Participants { participants })?;
        }

        ClientMessage::Join { .. } => {
            warn!("Received duplicate join message from peer {}", peer.id);
        }
    }

    Ok(())
}
