pub mod login;
pub mod logout;
pub mod status;

use async_trait::async_trait;
use clap::Subcommand;
use eyre::Result;

use super::Command;

#[derive(Subcommand, Clone)]
pub enum AuthCommands {
    /// Login to a provider using OAuth
    Login {
        /// Provider to login to
        provider: String,
    },
    /// Logout from a provider
    Logout {
        /// Provider to logout from
        provider: String,
    },
    /// Show authentication status for all providers
    Status,
}

pub struct AuthCommand {
    pub command: AuthCommands,
}

#[async_trait]
impl Command for AuthCommand {
    async fn execute(&self) -> Result<()> {
        match &self.command {
            AuthCommands::Login { provider } => login::execute(provider).await,
            AuthCommands::Logout { provider } => logout::execute(provider).await,
            AuthCommands::Status => status::execute().await,
        }
    }
}
