use serde::{Deserialize, Serialize};

/// Сообщения от клиента к серверу
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Присоединение к комнате
    #[serde(rename = "join")]
    Join {
        room: String,
        participant: String,
        name: String,
    },

    /// WebRTC Offer
    #[serde(rename = "offer")]
    Offer { sdp: String },

    /// WebRTC Answer (в некоторых случаях клиент может отправлять answer)
    #[serde(rename = "answer")]
    Answer { sdp: String },

    /// ICE кандидат
    #[serde(rename = "candidate")]
    Candidate { candidate: String },

    /// Обновление состояния участника (mute, video, screen sharing)
    #[serde(rename = "state_update")]
    StateUpdate {
        muted: bool,
        video_on: bool,
        screen_sharing: bool,
    },

    /// Запрос на начало screen sharing
    #[serde(rename = "start_screen_share")]
    StartScreenShare,

    /// Остановка screen sharing
    #[serde(rename = "stop_screen_share")]
    StopScreenShare,

    /// Ping для проверки соединения
    #[serde(rename = "ping")]
    Ping,

    /// Запрос списка участников
    #[serde(rename = "get_participants")]
    GetParticipants,
}

/// Сообщения от сервера к клиенту
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Подтверждение присоединения к комнате
    #[serde(rename = "joined")]
    Joined {
        your_id: String,
        participants: Vec<ParticipantInfo>,
    },

    /// WebRTC Answer
    #[serde(rename = "answer")]
    Answer { sdp: String },

    /// WebRTC Offer (для renegotiation)
    #[serde(rename = "offer")]
    Offer { sdp: String },

    /// ICE кандидат
    #[serde(rename = "candidate")]
    Candidate { candidate: String },

    /// Новый участник присоединился
    #[serde(rename = "participant_joined")]
    ParticipantJoined { id: String, name: String },

    /// Участник покинул комнату
    #[serde(rename = "participant_left")]
    ParticipantLeft { participant_id: String },

    /// Обновление состояния участника
    #[serde(rename = "state_update")]
    StateUpdate {
        participant_id: String,
        muted: bool,
        video_on: bool,
        screen_sharing: bool,
    },

    /// Список участников комнаты
    #[serde(rename = "participants")]
    Participants {
        participants: Vec<ParticipantInfo>,
    },

    /// Участник начал screen sharing
    #[serde(rename = "screen_share_started")]
    ScreenShareStarted { participant_id: String },

    /// Участник остановил screen sharing
    #[serde(rename = "screen_share_stopped")]
    ScreenShareStopped { participant_id: String },

    /// Pong ответ на ping
    #[serde(rename = "pong")]
    Pong,

    /// Сообщение об ошибке
    #[serde(rename = "error")]
    Error { message: String, code: Option<u32> },

    /// ICE gathering завершен
    #[serde(rename = "ice_gathering_complete")]
    IceGatheringComplete,
}

/// Информация об участнике
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ParticipantInfo {
    pub id: String,
    pub name: String,
    pub muted: bool,
    pub video_on: bool,
    pub screen_sharing: bool,
}

impl ParticipantInfo {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            muted: false,
            video_on: true,
            screen_sharing: false,
        }
    }

    pub fn with_state(
        id: String,
        name: String,
        muted: bool,
        video_on: bool,
        screen_sharing: bool,
    ) -> Self {
        Self {
            id,
            name,
            muted,
            video_on,
            screen_sharing,
        }
    }
}

/// Конфигурация ICE серверов для передачи клиенту
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IceServerConfig {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// Статистика для мониторинга
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RoomStats {
    pub room_id: String,
    pub participant_count: usize,
    pub active_tracks: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_join() {
        let msg = ClientMessage::Join {
            room: "room1".to_string(),
            participant: "user123".to_string(),
            name: "John Doe".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"join\""));
        assert!(json.contains("\"room\":\"room1\""));
    }

    #[test]
    fn test_deserialize_offer() {
        let json = r#"{"type":"offer","sdp":"v=0\r\n..."}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::Offer { sdp } => {
                assert_eq!(sdp, "v=0\r\n...");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_participant_info() {
        let info = ParticipantInfo::new("user1".to_string(), "Alice".to_string());
        assert_eq!(info.id, "user1");
        assert_eq!(info.name, "Alice");
        assert!(!info.muted);
        assert!(info.video_on);
        assert!(!info.screen_sharing);
    }

    #[test]
    fn test_serialize_server_message() {
        let msg = ServerMessage::Joined {
            your_id: "abc123".to_string(),
            participants: vec![],
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"joined\""));
    }
}
