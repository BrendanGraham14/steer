use async_trait::async_trait;
use eyre::Result;

pub mod auth;
pub mod headless;
pub mod init;
pub mod serve;
pub mod session;

#[async_trait]
pub trait Command {
    async fn execute(&self) -> Result<()>;
}
