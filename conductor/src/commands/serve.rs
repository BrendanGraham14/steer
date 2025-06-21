use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::info;

use super::Command;
use conductor_core::api::Model;
use conductor_core::session::SessionManagerConfig;

pub struct ServeCommand {
    pub port: u16,
    pub bind: String,
    pub model: Model,
}

#[async_trait]
impl Command for ServeCommand {
    async fn execute(&self) -> Result<()> {
        let addr = format!("{}:{}", self.bind, self.port)
            .parse()
            .map_err(|e| anyhow!("Invalid bind address: {}", e))?;

        info!("Starting gRPC server on {}", addr);

        // Create session store path
        let db_path = conductor_core::utils::session::create_session_store_path()?;

        let config = conductor_grpc::ServiceHostConfig {
            db_path,
            session_manager_config: SessionManagerConfig {
                max_concurrent_sessions: 100,
                default_model: self.model,
                auto_persist: true,
            },
            bind_addr: addr,
        };

        let mut host = conductor_grpc::ServiceHost::new(config).await?;
        host.start().await?;

        info!("gRPC server started on {}", addr);
        println!("Server listening on {}", addr);
        println!("Press Ctrl+C to shutdown");

        // Wait for shutdown signal
        tokio::signal::ctrl_c().await?;
        info!("Shutdown signal received");

        host.shutdown().await?;
        info!("Server shutdown complete");

        Ok(())
    }
}
