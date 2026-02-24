use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use tonic::transport::Server;
use tracing::{error, info, warn};

use steer_remote_workspace::proto::remote_workspace_service_server::RemoteWorkspaceServiceServer;
use steer_remote_workspace::remote_workspace_service::RemoteWorkspaceService;

const GRPC_MAX_MESSAGE_SIZE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Parser)]
#[command(name = "remote-workspace")]
#[command(about = "Remote workspace for Steer.")]
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

    let working_dir = match args.working_dir {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("Failed to read current directory for working dir: {error}"),
            )
        })?,
    };

    // Initialize logging
    let log_level = if args.debug { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(format!(
            "remote_workspace={log_level},remote_workspace={log_level}"
        ))
        .init();

    // Create the remote backend service
    let remote_workspace_service = RemoteWorkspaceService::new(working_dir)
        .await
        .map_err(|e| format!("Failed to create remote backend service: {e}"))?;

    info!(
        "Remote backend service created with {} supported tools",
        remote_workspace_service.get_supported_tools().len()
    );

    // Create the server
    let addr: SocketAddr = format!("{}:{}", args.address, args.port).parse()?;

    info!("Starting remote-workspace server on {}", addr);

    // Create a channel for graceful shutdown
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    // Handle shutdown signals
    let _signal_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        warn!("Received Ctrl+C, shutting down gracefully...");
        let _ = tx.send(());
    });

    // Start the server
    let server = Server::builder()
        .add_service(
            RemoteWorkspaceServiceServer::new(remote_workspace_service)
                .max_decoding_message_size(GRPC_MAX_MESSAGE_SIZE_BYTES)
                .max_encoding_message_size(GRPC_MAX_MESSAGE_SIZE_BYTES),
        )
        .serve_with_shutdown(addr, async {
            rx.await.ok();
        });

    info!("Remote workspace server ready to accept connections");

    if let Err(e) = server.await {
        error!(error = %e, "Server error");
        std::process::exit(1);
    }

    info!("Remote workspace server shut down gracefully");
    Ok(())
}
