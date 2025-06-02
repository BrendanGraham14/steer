use anyhow::Result;
use async_trait::async_trait;
use super::Command;

pub struct InitCommand {
    pub force: bool,
}

#[async_trait]
impl Command for InitCommand {
    async fn execute(&self) -> Result<()> {
        crate::config::init_config(self.force)?;
        println!("Configuration initialized successfully.");
        Ok(())
    }
}