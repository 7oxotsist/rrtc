use anyhow::Result;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use webrtc::track::track_remote::TrackRemote;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::TrackLocalWriter;

use crate::messages::ServerMessage;
use crate::peer::{Peer, TrackType};

/// Room представляет комнату с несколькими участниками
pub struct Room {
    pub id: String,
    peers: Arc<RwLock<HashMap<String, Arc<Peer>>>>,
}

impl Room {
    /// Создает новую комнату
    pub fn new(id: String) -> Self {
        info!("Creating new room: {}", id);
        Self {
            id,
            peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Добавляет участника в комнату
    pub async fn add_peer(&self, peer: Arc<Peer>) -> Result<()> {
        let peer_id = peer.id.clone();
        let peer_name = peer.name.clone();

        // Уведомляем существующих участников о новом
        let peers_guard = self.peers.read().await;
        for (_, existing_peer) in peers_guard.iter() {
            if let Err(e) = existing_peer.send_message(ServerMessage::ParticipantJoined {
                id: peer_id.clone(),
                name: peer_name.clone(),
            }) {
                warn!("Failed to notify peer {}: {}", existing_peer.id, e);
            }
        }
        drop(peers_guard);

        // Добавляем нового участника
        self.peers.write().await.insert(peer_id.clone(), peer);
        info!("Peer {} joined room {}", peer_id, self.id);

        Ok(())
    }

    /// Удаляет участника из комнаты
    pub async fn remove_peer(&self, peer_id: &str) -> Result<()> {
        let mut peers_guard = self.peers.write().await;

        if let Some(peer) = peers_guard.remove(peer_id) {
            info!("Removing peer {} from room {}", peer_id, self.id);

            // Закрываем соединение
            if let Err(e) = peer.close().await {
                warn!("Error closing peer connection: {}", e);
            }
        }

        // Уведомляем остальных участников
        let leave_msg = ServerMessage::ParticipantLeft {
            participant_id: peer_id.to_string(),
        };

        for (_, other_peer) in peers_guard.iter() {
            if let Err(e) = other_peer.send_message(leave_msg.clone()) {
                warn!("Failed to notify peer {}: {}", other_peer.id, e);
            }
        }

        Ok(())
    }

    /// Получает участника по ID
    pub async fn get_peer(&self, peer_id: &str) -> Option<Arc<Peer>> {
        self.peers.read().await.get(peer_id).cloned()
    }

    /// Получает всех участников
    pub async fn get_all_peers(&self) -> Vec<Arc<Peer>> {
        self.peers.read().await.values().cloned().collect()
    }

    /// Возвращает количество участников
    pub async fn peer_count(&self) -> usize {
        self.peers.read().await.len()
    }

    /// Проверяет, пуста ли комната
    pub async fn is_empty(&self) -> bool {
        self.peers.read().await.is_empty()
    }

    /// Транслирует сообщение всем участникам кроме отправителя
    pub async fn broadcast_message(&self, from_id: &str, msg: ServerMessage) {
        let peers_guard = self.peers.read().await;

        for (id, peer) in peers_guard.iter() {
            if id != from_id {
                if let Err(e) = peer.send_message(msg.clone()) {
                    warn!("Failed to broadcast to peer {}: {}", id, e);
                }
            }
        }
    }

    /// Транслирует сообщение всем участникам включая отправителя
    pub async fn broadcast_message_to_all(&self, msg: ServerMessage) {
        let peers_guard = self.peers.read().await;

        for (_, peer) in peers_guard.iter() {
            if let Err(e) = peer.send_message(msg.clone()) {
                warn!("Failed to broadcast to peer {}: {}", peer.id, e);
            }
        }
    }

    /// Обрабатывает входящий трек от участника и маршрутизирует его другим
    pub async fn handle_incoming_track(
        &self,
        from_peer_id: String,
        track: Arc<TrackRemote>,
    ) -> Result<()> {
        let track_type = TrackType::from_track_id(&track.id());

        info!(
            "Room {}: Handling incoming {:?} track from peer {} (id: {}, kind: {:?})",
            self.id,
            track_type,
            from_peer_id,
            track.id(),
            track.kind()
        );

        // Запускаем задачу для чтения и пересылки RTP пакетов
        let room_id = self.id.clone();
        let peers = self.peers.clone();
        let from_id = from_peer_id.clone();

        tokio::spawn(async move {
            if let Err(e) = relay_track(room_id, peers, from_id, track, track_type).await {
                error!("Error relaying track: {}", e);
            }
        });

        Ok(())
    }

    /// Получает статистику комнаты
    pub async fn get_stats(&self) -> String {
        let peers_guard = self.peers.read().await;
        let peer_count = peers_guard.len();

        let mut stats = format!("Room {} - {} peers:\n", self.id, peer_count);

        for (_, peer) in peers_guard.iter() {
            let peer_stats = peer.get_stats().await;
            stats.push_str(&format!("  {}\n", peer_stats));
        }

        stats
    }
}

/// Пересылает RTP пакеты от одного участника всем остальным
async fn relay_track(
    room_id: String,
    peers: Arc<RwLock<HashMap<String, Arc<Peer>>>>,
    from_id: String,
    track: Arc<TrackRemote>,
    track_type: TrackType,
) -> Result<()> {
    let mut buf = vec![0u8; 1500];
    let mut packet_count = 0u64;

    loop {
        // Читаем RTP пакет из входящего трека
        let (rtp_packet, _attributes) = match track.read(&mut buf).await {
            Ok(result) => result,
            Err(e) => {
                warn!(
                    "Error reading from track {} in room {}: {}",
                    track.id(),
                    room_id,
                    e
                );
                break;
            }
        };

        packet_count += 1;

        // Каждые 1000 пакетов логируем для отладки
        if packet_count % 1000 == 0 {
            debug!(
                "Relayed {} packets for track {:?} from peer {} in room {}",
                packet_count, track_type, from_id, room_id
            );
        }

        // Получаем список участников для пересылки
        let peers_guard = peers.read().await;

        for (peer_id, peer) in peers_guard.iter() {
            // Не отправляем трек обратно отправителю
            if peer_id == &from_id {
                continue;
            }

            // Проверяем состояние участника
            let (muted, video_on, screen_sharing) = peer.get_state().await;

            // Фильтруем треки на основе состояния
            match track_type {
                TrackType::Audio => {
                    if muted {
                        continue;
                    }
                }
                TrackType::Camera => {
                    if !video_on {
                        continue;
                    }
                }
                TrackType::Screen => {
                    // Экран всегда пересылается если screen_sharing активен
                    if !screen_sharing {
                        continue;
                    }
                }
            }

            // Ищем соответствующий локальный трек для отправки
            let local_tracks = peer.local_tracks.read().await;

            for local_track_info in local_tracks.iter() {
                // Проверяем, совпадает ли тип трека и codec type
                if local_track_info.track_type == track_type
                    && local_track_info.track.kind() == track.kind()
                {
                    // Отправляем RTP пакет
                    if let Err(e) = local_track_info.track.write_rtp(&rtp_packet).await {
                        if e.to_string().contains("InvalidState") {
                            // Соединение закрыто, это нормально
                            debug!("Track closed for peer {}", peer_id);
                        } else {
                            warn!(
                                "Error writing RTP to peer {} track {:?}: {}",
                                peer_id, track_type, e
                            );
                        }
                    }
                    break;
                }
            }
        }
    }

    info!(
        "Track relay stopped for {:?} from peer {} in room {} (total packets: {})",
        track_type, from_id, room_id, packet_count
    );

    Ok(())
}

/// Менеджер комнат
pub struct RoomManager {
    rooms: Arc<RwLock<HashMap<String, Arc<Room>>>>,
}

impl RoomManager {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Получает или создает комнату
    pub async fn get_or_create_room(&self, room_id: String) -> Arc<Room> {
        let rooms_guard = self.rooms.read().await;

        if let Some(room) = rooms_guard.get(&room_id) {
            return room.clone();
        }

        drop(rooms_guard);

        // Создаем новую комнату
        let room = Arc::new(Room::new(room_id.clone()));
        self.rooms.write().await.insert(room_id, room.clone());

        room
    }

    /// Получает комнату по ID
    pub async fn get_room(&self, room_id: &str) -> Option<Arc<Room>> {
        self.rooms.read().await.get(room_id).cloned()
    }

    /// Удаляет комнату, если она пуста
    pub async fn cleanup_empty_room(&self, room_id: &str) -> bool {
        if let Some(room) = self.get_room(room_id).await {
            if room.is_empty().await {
                self.rooms.write().await.remove(room_id);
                info!("Removed empty room: {}", room_id);
                return true;
            }
        }
        false
    }

    /// Получает количество комнат
    pub async fn room_count(&self) -> usize {
        self.rooms.read().await.len()
    }

    /// Получает общую статистику
    pub async fn get_stats(&self) -> String {
        let rooms_guard = self.rooms.read().await;
        let room_count = rooms_guard.len();

        let mut stats = format!("RoomManager - {} rooms:\n", room_count);

        for (_, room) in rooms_guard.iter() {
            stats.push_str(&room.get_stats().await);
        }

        stats
    }

    /// Очищает все пустые комнаты
    pub async fn cleanup_all_empty_rooms(&self) {
        let room_ids: Vec<String> = self.rooms.read().await.keys().cloned().collect();

        for room_id in room_ids {
            self.cleanup_empty_room(&room_id).await;
        }
    }
}

impl Default for RoomManager {
    fn default() -> Self {
        Self::new()
    }
}
