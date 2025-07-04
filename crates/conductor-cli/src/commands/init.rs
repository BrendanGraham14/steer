use super::Command;
use async_trait::async_trait;
use eyre::Result;

pub struct InitCommand {
    pub force: bool,
}

#[async_trait]
impl Command for InitCommand {
    async fn execute(&self) -> Result<()> {
        crate::config::init_config(self.force)
            .map_err(|e| eyre::eyre!("Failed to initialize config: {}", e))?;
        println!("Configuration initialized successfully.");
        Ok(())
    }
}
