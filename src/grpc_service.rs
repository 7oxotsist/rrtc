// grpc_service.rs (без изменений)
use std::sync::Arc;
use tonic::{Request, Response, Status, Streaming};
use tokio_stream::wrappers::ReceiverStream;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use crate::sfu::{
    sfu_control_server::SfuControl,
    CreateRoomRequest, CreateRoomResponse,
    JoinRoomRequest, JoinRoomResponse,
    SignalMessage,
};
use crate::state::{RoomManager, SessionManager, MediaPortManager};

#[derive(Clone)]
pub struct SfuGrpcService {
    pub room_manager: RoomManager,
    pub session_manager: Arc<SessionManager>,
    pub media_port_manager: MediaPortManager,
}

#[tonic::async_trait]
impl SfuControl for SfuGrpcService {
    type SignalStream = ReceiverStream<Result<SignalMessage, Status>>;

    async fn create_room(&self, request: Request<CreateRoomRequest>) -> Result<Response<CreateRoomResponse>, Status> {
        let room_id = request.into_inner().room_id;
        self.room_manager.create_room(room_id.clone()).await;

        Ok(Response::new(CreateRoomResponse {
            success: true,
            message: format!("Room {} created", room_id),
        }))
    }

    async fn join_room(&self, request: Request<JoinRoomRequest>) -> Result<Response<JoinRoomResponse>, Status> {
        let req = request.into_inner();
        let room_id = req.room_id;

        if !self.room_manager.room_exists(&room_id).await {
            return Ok(Response::new(JoinRoomResponse {
                success: false,
                message: "Room not found".into(),
                ..Default::default()
            }));
        }

        let sid = req.sid;

        let media_port = self.media_port_manager.allocate_port().await
            .ok_or(Status::internal("No available media ports"))?;

        self.session_manager.create_session(sid.clone(), room_id.clone(), media_port).await
            .map_err(|_| Status::internal("Failed to create session"))?;

        self.room_manager.add_participant(room_id, sid.clone()).await;

        Ok(Response::new(JoinRoomResponse {
            success: true,
            sid,
            media_port: media_port as u32,
            message: "Joined successfully".into(),
        }))
    }

    async fn signal(&self, request: Request<Streaming<SignalMessage>>) -> Result<Response<Self::SignalStream>, Status> {
        let mut stream = request.into_inner();

        let (server_tx, server_rx) = mpsc::channel(100);

        let session_manager = self.session_manager.clone();
        let media_port_manager = self.media_port_manager.clone();

        tokio::spawn(async move {
            let mut current_sid: Option<String> = None;

            while let Some(result) = stream.next().await {
                let msg = match result {
                    Ok(m) => m,
                    Err(_) => break,
                };

                let sid = msg.sid.clone();

                if current_sid.is_none() {
                    current_sid = Some(sid.clone());
                    let _ = session_manager.set_response_tx(&sid, server_tx..clone()).await;
                }

                if let Some(session) = session_manager.get_session(&sid).await {
                    if let Some(media_tx) = media_port_manager.get_tx(session.media_port).await {
                        let _ = media_tx.send(msg).await;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(server_rx)))
    }
}