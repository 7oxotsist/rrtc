use std::sync::Arc;
use grpc_service::SfuGrpcService;
use media_worker::spawn_media_worker;
use state::{RoomManager, SessionManager, MediaPortManager};
use tonic::{transport::Server};
use async_channel::unbounded;
use tonic_reflection::server::{Builder as ReflectionBuilder, v1::ServerReflectionServer};

mod state;
mod grpc_service;
mod media_worker;
pub mod sfu {
    tonic::include_proto!("sfu");
    pub(crate) const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!("./file_descriptor_set.bin");
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::]:50052".parse()?;

    let room_manager = RoomManager::new();
    let session_manager = Arc::new(SessionManager::new());
    let media_port_manager = MediaPortManager::new();

    // Диапазон медиа-портов
    let media_ports = 3478..=3495;

    for port in media_ports {
        let (tx, rx) = unbounded();
        media_port_manager.register_port(port, tx).await;
        spawn_media_worker(port, rx, session_manager.clone()).await;
    }

    let service = SfuGrpcService {
        room_manager,
        session_manager,
        media_port_manager,
    };

    // === Reflection сервис ===
    let reflection_service = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(sfu::FILE_DESCRIPTOR_SET)
        .build_v1()  // или build_v1alpha() для старых клиентов
        .unwrap();

    println!("Rust SFU gRPC server listening on {}", addr);

    Server::builder()
        .add_service(sfu::sfu_control_server::SfuControlServer::new(service))
        .add_service(reflection_service)
        .serve(addr)
        .await?;

    Ok(())
}