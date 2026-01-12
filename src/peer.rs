use anyhow::Result;
use log::{debug, info};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::Message;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocal;
use interceptor::registry::Registry;

use crate::messages::ServerMessage;

/// Типы треков для различения камеры и экрана
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackType {
    Camera,
    Screen,
    Audio,
}

impl TrackType {
    pub fn from_track_id(track_id: &str) -> Self {
        if track_id.contains("screen") {
            TrackType::Screen
        } else if track_id.contains("audio") {
            TrackType::Audio
        } else {
            TrackType::Camera
        }
    }
}

/// Информация о локальном треке для отправки другим участникам
#[derive(Clone)]
pub struct LocalTrack {
    pub track: Arc<TrackLocalStaticRTP>,
    pub sender: Arc<RTCRtpSender>,
    pub track_type: TrackType,
}

/// Peer представляет одного участника в комнате
#[derive(Clone)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub pc: Arc<RTCPeerConnection>,
    pub ws_tx: mpsc::UnboundedSender<Message>,
    pub muted: Arc<RwLock<bool>>,
    pub video_on: Arc<RwLock<bool>>,
    pub screen_sharing: Arc<RwLock<bool>>,
    pub local_tracks: Arc<RwLock<Vec<LocalTrack>>>,
}

impl Peer {
    /// Создает новый Peer с настроенным WebRTC API и ICE серверами
    pub async fn new(
        id: String,
        name: String,
        ws_tx: mpsc::UnboundedSender<Message>,
        ice_servers: Option<Vec<RTCIceServer>>,
    ) -> Result<Self> {
        // Настройка Media Engine
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;

        // Настройка Interceptor Registry
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)?;

        // Создание API
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        // Конфигурация ICE серверов
        let ice_servers = ice_servers.unwrap_or_else(|| {
            vec![
                RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                    ..Default::default()
                },
                RTCIceServer {
                    urls: vec!["stun:stun1.l.google.com:19302".to_owned()],
                    ..Default::default()
                },
                // Здесь можно добавить TURN сервера
                // RTCIceServer {
                //     urls: vec!["turn:turn.example.com:3478".to_owned()],
                //     username: "username".to_owned(),
                //     credential: "password".to_owned(),
                //     credential_type: RTCIceCredentialType::Password,
                // },
            ]
        });

        let config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };

        let peer_connection = Arc::new(api.new_peer_connection(config).await?);

        info!("Created peer connection for {}", id);

        Ok(Peer {
            id,
            name,
            pc: peer_connection,
            ws_tx,
            muted: Arc::new(RwLock::new(false)),
            video_on: Arc::new(RwLock::new(true)),
            screen_sharing: Arc::new(RwLock::new(false)),
            local_tracks: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Настраивает обработчики событий для PeerConnection
    pub async fn setup_handlers(&self) -> Result<()> {
        let peer_id = self.id.clone();
        let ws_tx = self.ws_tx.clone();

        // Обработчик ICE кандидатов
        self.pc
            .on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
                let tx = ws_tx.clone();
                let peer_id = peer_id.clone();
                Box::pin(async move {
                    if let Some(c) = candidate {
                        debug!("Peer {} generated ICE candidate", peer_id);
                        if let Ok(json) = c.to_json() {
                            let msg = ServerMessage::Candidate {
                                candidate: json.candidate,
                            };
                            if let Ok(json_str) = serde_json::to_string(&msg) {
                                let _ = tx.send(Message::text(json_str));
                            }
                        }
                    } else {
                        debug!("Peer {} ICE gathering complete", peer_id);
                    }
                })
            }));


        // Обработчик состояния соединения
        let peer_id_clone = self.id.clone();
        self.pc.on_peer_connection_state_change(Box::new(
            move |state: RTCPeerConnectionState| {
                info!("Peer {} connection state: {:?}", peer_id_clone, state);
                Box::pin(async {})
            },
        ));

        // Обработчик ICE connection state
        let peer_id_clone2 = self.id.clone();
        self.pc
            .on_ice_connection_state_change(Box::new(move |state: RTCIceConnectionState| {
                info!("Peer {} ICE connection state: {:?}", peer_id_clone2, state);
                Box::pin(async {})
            }));

        Ok(())
    }

    /// Добавляет локальный трек для отправки медиа другим участникам
    pub async fn add_local_track(
        &self,
        codec: &str,
        track_id: &str,
        track_type: TrackType,
    ) -> Result<Arc<TrackLocalStaticRTP>> {
        let track_id_owned = track_id.to_string();
        let (mime_type, _kind) = match track_type {
            TrackType::Audio => ("audio/opus", RTPCodecType::Audio),
            TrackType::Camera | TrackType::Screen => (codec, RTPCodecType::Video),
        };

        let track = Arc::new(TrackLocalStaticRTP::new(
            webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: mime_type.to_owned(),
                ..Default::default()
            },
            track_id.to_owned(),
            format!("webrtc-rs-{}", self.id),
        ));

        let rtp_sender = self
            .pc
            .add_track(track.clone() as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        // Клонируем для сохранения
        let rtp_sender_clone = rtp_sender.clone();

        // Читаем RTCP пакеты (для keep-alive)
        let peer_id = self.id.clone();
        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {
                // Обработка RTCP пакетов
            }
            debug!("RTCP reader for peer {} track {} stopped", peer_id, track_id_owned);
        });

        // Сохраняем информацию о треке
        self.local_tracks.write().await.push(LocalTrack {
            track: track.clone(),
            sender: rtp_sender_clone,
            track_type,
        });

        info!(
            "Added local track {:?} for peer {}: {}",
            track_type, self.id, track_id
        );

        Ok(track)
    }

    /// Обрабатывает offer от клиента и создает answer
    pub async fn handle_offer(&self, sdp: String) -> Result<String> {
        let offer = RTCSessionDescription::offer(sdp)?;
        self.pc.set_remote_description(offer).await?;

        let answer = self.pc.create_answer(None).await?;
        let answer_sdp = answer.sdp.clone();
        self.pc.set_local_description(answer).await?;

        info!("Created answer for peer {}", self.id);
        Ok(answer_sdp)
    }

    /// Добавляет ICE кандидата
    pub async fn add_ice_candidate(&self, candidate: String) -> Result<()> {
        let ice_candidate = RTCIceCandidateInit {
            candidate,
            ..Default::default()
        };
        self.pc.add_ice_candidate(ice_candidate).await?;
        debug!("Added ICE candidate for peer {}", self.id);
        Ok(())
    }

    /// Обновляет состояние участника
    pub async fn update_state(&self, muted: bool, video_on: bool, screen_sharing: bool) {
        *self.muted.write().await = muted;
        *self.video_on.write().await = video_on;
        *self.screen_sharing.write().await = screen_sharing;

        info!(
            "Peer {} state updated: muted={}, video={}, screen={}",
            self.id, muted, video_on, screen_sharing
        );
    }

    /// Получает текущее состояние участника
    pub async fn get_state(&self) -> (bool, bool, bool) {
        let muted = *self.muted.read().await;
        let video_on = *self.video_on.read().await;
        let screen_sharing = *self.screen_sharing.read().await;
        (muted, video_on, screen_sharing)
    }

    /// Отправляет сообщение участнику через WebSocket
    pub fn send_message(&self, msg: ServerMessage) -> Result<()> {
        let json = serde_json::to_string(&msg)?;
        self.ws_tx
            .send(Message::text(json))
            .map_err(|e| anyhow::anyhow!("Failed to send message: {}", e))?;
        Ok(())
    }

    /// Закрывает peer connection
    pub async fn close(&self) -> Result<()> {
        self.pc.close().await?;
        info!("Closed peer connection for {}", self.id);
        Ok(())
    }

    /// Получает статистику соединения (для отладки)
    pub async fn get_stats(&self) -> String {
        let state = self.pc.connection_state();
        let ice_state = self.pc.ice_connection_state();
        let ice_gathering_state = self.pc.ice_gathering_state();

        format!(
            "Peer {}: state={:?}, ice={:?}, gathering={:?}",
            self.id, state, ice_state, ice_gathering_state
        )
    }
}

/// Builder для создания Peer с кастомными настройками
pub struct PeerBuilder {
    id: String,
    name: String,
    ws_tx: mpsc::UnboundedSender<Message>,
    ice_servers: Option<Vec<RTCIceServer>>,
}

impl PeerBuilder {
    pub fn new(id: String, name: String, ws_tx: mpsc::UnboundedSender<Message>) -> Self {
        Self {
            id,
            name,
            ws_tx,
            ice_servers: None,
        }
    }

    pub fn with_ice_servers(mut self, servers: Vec<RTCIceServer>) -> Self {
        self.ice_servers = Some(servers);
        self
    }

    pub fn with_turn_server(
        mut self,
        url: String,
        username: String,
        credential: String,
    ) -> Self {
        let turn_server = RTCIceServer {
            urls: vec![url],
            username,
            credential,
            credential_type: webrtc::ice_transport::ice_credential_type::RTCIceCredentialType::Password,
        };

        match &mut self.ice_servers {
            Some(servers) => servers.push(turn_server),
            None => self.ice_servers = Some(vec![turn_server]),
        }

        self
    }

    pub async fn build(self) -> Result<Peer> {
        let peer = Peer::new(self.id, self.name, self.ws_tx, self.ice_servers).await?;
        peer.setup_handlers().await?;
        Ok(peer)
    }
}
