use async_trait::async_trait;
use eyre::Result;

pub mod headless;
pub mod preferences;
pub mod serve;
pub mod session;
pub mod workspace;

#[async_trait]
pub trait Command {
    async fn execute(&self) -> Result<()>;
}
