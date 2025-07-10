use super::Command;
use crate::error::Error;
use async_trait::async_trait;
use conductor_core::preferences::Preferences;
use eyre::Result;

pub struct PreferencesCommand {
    pub action: PreferencesAction,
}

pub enum PreferencesAction {
    Show,
    Edit,
    Reset,
}

#[async_trait]
impl Command for PreferencesCommand {
    async fn execute(&self) -> Result<()> {
        match &self.action {
            PreferencesAction::Show => self.show().await.map_err(Into::into),
            PreferencesAction::Edit => self.edit().await.map_err(Into::into),
            PreferencesAction::Reset => self.reset().await.map_err(Into::into),
        }
    }
}

impl PreferencesCommand {
    async fn show(&self) -> std::result::Result<(), Error> {
        let prefs = Preferences::load()?;
        let path = Preferences::config_path()?;

        println!("Preferences file: {}", path.display());
        println!("\n{}", toml::to_string_pretty(&prefs)?);
        Ok(())
    }

    async fn edit(&self) -> std::result::Result<(), Error> {
        let path = Preferences::config_path()?;

        // Ensure the file exists
        if !path.exists() {
            let prefs = Preferences::default();
            prefs.save()?;
        }

        // Open in default editor
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| {
                if cfg!(target_os = "windows") {
                    "notepad".to_string()
                } else {
                    "vi".to_string()
                }
            });

        let status = std::process::Command::new(&editor).arg(&path).status()?;

        if !status.success() {
            return Err(Error::Process(format!(
                "Editor '{editor}' exited with status: {status}"
            )));
        }

        Ok(())
    }

    async fn reset(&self) -> std::result::Result<(), Error> {
        let path = Preferences::config_path()?;

        if path.exists() {
            std::fs::remove_file(&path)?;
            println!("Preferences reset to defaults");
        } else {
            println!("No preferences file found");
        }
        Ok(())
    }
}
