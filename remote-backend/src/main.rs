use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use tonic::transport::Server;
use tracing::{info, warn};

mod remote_backend_service;

use remote_backend_service::RemoteBackendService;

// Include the generated protobuf code
pub mod proto {
    tonic::include_proto!("coder.remote_backend.v1");
}

use proto::remote_backend_service_server::RemoteBackendServiceServer;

#[derive(Parser)]
#[command(name = "remote-backend")]
#[command(about = "Coder remote backend for tool execution")]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value = "50051")]
    port: u16,

    /// Address to bind to
    #[arg(short, long, default_value = "0.0.0.0")]
    address: String,

    /// Working directory for tool execution
    #[arg(short, long)]
    working_dir: Option<PathBuf>,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.debug { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(format!(
            "coder_remote_backend={},coder={}",
            log_level, log_level
        ))
        .init();

    // Change working directory if specified
    if let Some(working_dir) = &args.working_dir {
        std::env::set_current_dir(working_dir)?;
        info!("Changed working directory to: {}", working_dir.display());
    }

    // Create the remote backend service
    let remote_backend_service = RemoteBackendService::new()
        .map_err(|e| format!("Failed to create remote backend service: {}", e))?;

    info!(
        "Remote backend service created with {} supported tools",
        remote_backend_service.get_supported_tools().len()
    );

    // Create the server
    let addr: SocketAddr = format!("{}:{}", args.address, args.port).parse()?;

    info!("Starting remote-backend server on {}", addr);
    info!("Working directory: {}", std::env::current_dir()?.display());

    // Create a channel for graceful shutdown
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    // Handle shutdown signals
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        warn!("Received Ctrl+C, shutting down gracefully...");
        let _ = tx.send(());
    });

    // Start the server
    let server = Server::builder()
        .add_service(RemoteBackendServiceServer::new(remote_backend_service))
        .serve_with_shutdown(addr, async {
            rx.await.ok();
        });

    info!("Remote backend server ready to accept connections");

    if let Err(e) = server.await {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    }

    info!("Remote backend server shut down gracefully");
    Ok(())
}
