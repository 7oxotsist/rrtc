// media_worker.rs (без параметра meter и без метрик)
use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use async_broadcast::Receiver as BroadcastReceiver;
use log::{info, error};
use retty::bootstrap::BootstrapUdpServer;
use retty::channel::Pipeline;
use retty::transport::TaggedBytesMut;
use crate::sfu::{
    DataChannelHandler, DemuxerHandler, DtlsHandler, ExceptionHandler, GatewayHandler,
    InterceptorHandler, ServerConfig, ServerStates, SctpHandler, SrtpHandler, StunHandler,
};
use crate::state::{MediaPortManager, SessionManager};

pub async fn spawn_media_worker(
    host: String,
    port: u16,
    server_config: Arc<ServerConfig>,
    mut stop_rx: BroadcastReceiver<()>,
    session_manager: Arc<SessionManager>,
    media_port_manager: MediaPortManager,
) {
    let local_addr: SocketAddr = SocketAddr::from_str(&format!("{}:{}", host, port)).unwrap();

    info!("Spawning media worker on {}", local_addr);

    // Предполагаем, что ServerStates::new принимает только server_config и local_addr
    // (если в вашем crate есть meter — удалите его или сделайте опциональным)
    let server_states = Rc::new(RefCell::new(
        ServerStates::new(server_config.clone(), local_addr)
            .expect("Failed to create ServerStates"),
    ));

    let mut bootstrap = BootstrapUdpServer::new();

    let server_states_for_pipeline = server_states.clone();

    bootstrap.pipeline(Box::new(move || {
        let pipeline: Pipeline<TaggedBytesMut, TaggedBytesMut> = Pipeline::new();

        pipeline.add_back(DemuxerHandler::new());
        pipeline.add_back(StunHandler::new());
        pipeline.add_back(DtlsHandler::new(local_addr, server_states_for_pipeline.clone()));
        pipeline.add_back(SctpHandler::new(local_addr, server_states_for_pipeline.clone()));
        pipeline.add_back(DataChannelHandler::new());
        pipeline.add_back(SrtpHandler::new(server_states_for_pipeline.clone()));
        pipeline.add_back(InterceptorHandler::new(server_states_for_pipeline.clone()));
        pipeline.add_back(GatewayHandler::new(server_states_for_pipeline.clone()));
        pipeline.add_back(ExceptionHandler::new());

        pipeline.finalize()
    }));

    if let Err(e) = bootstrap.bind(local_addr).await {
        error!("Failed to bind UDP on {}: {}", local_addr, e);
        return;
    }

    info!("UDP server successfully bound on {}", local_addr);

    loop {
        tokio::select! {
            _ = stop_rx.recv() => {
                info!("Media worker on {} received stop signal", local_addr);
                break;
            }
        }
    }

    bootstrap.graceful_stop().await;
    info!("Media worker on {} stopped gracefully", local_addr);
}