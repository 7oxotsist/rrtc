use async_channel::{unbounded, Sender, Receiver};
use crate::sfu::SignalMessage;
use crate::state::{SessionManager, SessionInfo};
use std::sync::Arc;
use tokio;

pub async fn spawn_media_worker(
    port: u16,
    rx: Receiver<SignalMessage>,
    session_manager: Arc<SessionManager>,
) {
    tokio::spawn(async move {
        println!("Media worker started on port {}", port);

        while let Ok(msg) = rx.recv().await {
            let sid = msg.sid.clone();

            // Здесь вставь свой существующий код обработки (DTLS, SRTP, SDP и т.д.)
            // Пример: если нужно отправить answer/candidate клиенту
            if let Some(session) = session_manager.get_session(&sid).await {
                // session.client_tx.send(answer_msg);
            }

            // Твоя логика из sfu crate / retty
        }

        println!("Media worker on port {} stopped", port);
    });
}