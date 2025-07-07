use async_trait::async_trait;
use eyre::{Result, eyre};
use tracing::info;

use super::Command;
use conductor_core::api::Model;
use conductor_core::session::SessionManagerConfig;

pub struct ServeCommand {
    pub port: u16,
    pub bind: String,
    pub model: Model,
    pub session_db: Option<std::path::PathBuf>,
}

#[async_trait]
impl Command for ServeCommand {
    async fn execute(&self) -> Result<()> {
        let addr = format!("{}:{}", self.bind, self.port)
            .parse()
            .map_err(|e| eyre!("Invalid bind address: {}", e))?;

        info!("Starting gRPC server on {}", addr);

        // Resolve session store path from config or use default
        let db_path = match &self.session_db {
            Some(path) => path.clone(),
            None => conductor_core::utils::session::create_session_store_path()?,
        };

        let config = conductor_grpc::ServiceHostConfig::new(
            db_path,
            SessionManagerConfig {
                max_concurrent_sessions: 100,
                default_model: self.model,
                auto_persist: true,
            },
            addr,
        )
        .map_err(|e| eyre!("Failed to create service config: {}", e))?;

        let mut host = conductor_grpc::ServiceHost::new(config)
            .await
            .map_err(|e| eyre!("Failed to create service host: {}", e))?;
        host.start()
            .await
            .map_err(|e| eyre!("Failed to start server: {}", e))?;

        info!("gRPC server started on {}", addr);
        println!("Server listening on {}", addr);
        println!("Press Ctrl+C to shutdown");

        // Wait for shutdown signal
        tokio::signal::ctrl_c().await?;
        info!("Shutdown signal received");

        host.shutdown()
            .await
            .map_err(|e| eyre!("Failed to shutdown server: {}", e))?;
        info!("Server shutdown complete");

        Ok(())
    }
}
