pub mod import;
pub mod login;
pub mod logout;
pub mod status;

use async_trait::async_trait;
use clap::Subcommand;
use eyre::Result;

use self::{import::Import, login::Login, logout::Logout};

use super::Command;

#[derive(Subcommand, Clone, Debug)]
pub enum AuthCommands {
    /// Login to an authentication provider
    Login(Login),
    /// Logout from an authentication provider
    Logout(Logout),
    /// Show authentication status
    Status,
    /// Import an existing API key for a provider and store it securely in your local keyring.
    Import(Import),
}

pub struct AuthCommand {
    pub command: AuthCommands,
}

#[async_trait]
impl Command for AuthCommand {
    async fn execute(&self) -> Result<()> {
        match self.command.clone() {
            AuthCommands::Login(login) => login.handle().await,
            AuthCommands::Logout(logout) => logout.handle().await,
            AuthCommands::Status => status::execute().await,
            AuthCommands::Import(import) => import.handle().await,
        }
    }
}
