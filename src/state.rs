use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use async_channel::Sender;
use crate::sfu::SignalMessage;

#[derive(Clone)]
pub struct RoomManager {
    rooms: Arc<Mutex<HashMap<String, Vec<String>>>>, // room_id → vec<sids>
}

impl RoomManager {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn create_room(&self, room_id: String) {
        let mut rooms = self.rooms.lock().await;
        rooms.entry(room_id).or_insert_with(Vec::new);
    }

    pub async fn room_exists(&self, room_id: &str) -> bool {
        let rooms = self.rooms.lock().await;
        rooms.contains_key(room_id)
    }

    pub async fn add_participant(&self, room_id: String, sid: String) {
        let mut rooms = self.rooms.lock().await;
        if let Some(participants) = rooms.get_mut(&room_id) {
            participants.push(sid);
        }
    }
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<String, SessionInfo>>>, // sid → info
}

#[derive(Clone)]
pub struct SessionInfo {
    pub room_id: String,
    pub media_port: u16,
    pub client_tx: tokio::sync::mpsc::UnboundedSender<SignalMessage>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn create_session(&self, sid: String, room_id: String, media_port: u16) -> tokio::sync::mpsc::UnboundedSender<SignalMessage> {
        let (tx, _) = tokio::sync::mpsc::unbounded_channel::<SignalMessage>();
        let info = SessionInfo {
            room_id,
            media_port,
            client_tx: tx.clone(),
        };
        let mut sessions = self.sessions.lock().await;
        sessions.insert(sid, info);
        tx
    }

    pub async fn get_session(&self, sid: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions.get(sid).cloned()
    }

    pub async fn remove_session(&self, sid: &str) {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(sid);
    }
}

#[derive(Clone)]
pub struct MediaPortManager {
    media_tx_map: Arc<Mutex<HashMap<u16, Sender<SignalMessage>>>>,
}

impl MediaPortManager {
    pub fn new() -> Self {
        Self {
            media_tx_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_port(&self, port: u16, tx: Sender<SignalMessage>) {
        let mut map = self.media_tx_map.lock().await;
        map.insert(port, tx);
    }

    pub async fn get_tx(&self, port: u16) -> Option<Sender<SignalMessage>> {
        let map = self.media_tx_map.lock().await;
        map.get(&port).cloned()
    }

    // Простой выбор порта (можно улучшить: round-robin, по нагрузке)
    pub async fn allocate_port(&self) -> Option<u16> {
        let map = self.media_tx_map.lock().await;
        map.keys().next().copied()
    }
}