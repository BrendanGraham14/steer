use anyhow::Result;
use async_trait::async_trait;

pub mod init;
pub mod headless;
pub mod serve;
pub mod session;

#[async_trait]
pub trait Command {
    async fn execute(&self) -> Result<()>;
}