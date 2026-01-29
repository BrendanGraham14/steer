use super::Command;
use crate::error::Error;
use async_trait::async_trait;
use eyre::Result;
use shell_words::split;
use std::io::Write;
use std::process::Command as ProcessCommand;
use steer_core::preferences::Preferences;

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

        let mut stdout = std::io::stdout();
        writeln!(stdout, "Preferences file: {}", path.display())?;
        writeln!(stdout, "\n{}", toml::to_string_pretty(&prefs)?)?;
        Ok(())
    }

    async fn edit(&self) -> std::result::Result<(), Error> {
        let path = Preferences::config_path()?;

        // Ensure the file exists
        if !path.exists() {
            let prefs = Preferences::default();
            prefs.save()?;
        }

        let (editor, mut args) = Self::parse_editor_command()?;
        args.push(path.to_string_lossy().to_string());

        let status = ProcessCommand::new(&editor)
            .args(&args)
            .status()
            .map_err(|err| {
                Error::Process(format!(
                    "Failed to launch editor '{editor}': {err}. Set $VISUAL or $EDITOR to a valid editor."
                ))
            })?;

        if !status.success() {
            return Err(Error::Process(format!(
                "Editor '{editor}' exited with status: {status}"
            )));
        }

        Ok(())
    }

    fn parse_editor_command() -> std::result::Result<(String, Vec<String>), Error> {
        let editor = std::env::var("VISUAL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                std::env::var("EDITOR")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .unwrap_or_else(|| {
                if cfg!(target_os = "windows") {
                    "notepad".to_string()
                } else {
                    "vi".to_string()
                }
            });

        Self::parse_editor_command_str(&editor)
    }

    fn parse_editor_command_str(editor: &str) -> std::result::Result<(String, Vec<String>), Error> {
        let parts = split(editor).map_err(|err| {
            Error::Process(format!(
                "Failed to parse editor command '{editor}': {err}. Set $VISUAL or $EDITOR to a valid editor."
            ))
        })?;

        let Some((command, args)) = parts.split_first() else {
            return Err(Error::Process(
                "Editor command is empty. Set $VISUAL or $EDITOR to a valid editor.".to_string(),
            ));
        };

        Ok((command.to_string(), args.to_vec()))
    }

    async fn reset(&self) -> std::result::Result<(), Error> {
        let path = Preferences::config_path()?;

        if path.exists() {
            std::fs::remove_file(&path)?;
            let mut stdout = std::io::stdout();
            writeln!(stdout, "Preferences reset to defaults")?;
        } else {
            let mut stdout = std::io::stdout();
            writeln!(stdout, "No preferences file found")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PreferencesCommand;

    #[test]
    fn parses_editor_with_arguments() {
        let (command, args) =
            PreferencesCommand::parse_editor_command_str(r"code --wait --new-window").unwrap();
        assert_eq!(command, "code");
        assert_eq!(args, vec!["--wait", "--new-window"]);
    }

    #[test]
    fn parses_editor_without_arguments() {
        let (command, args) = PreferencesCommand::parse_editor_command_str("vim").unwrap();
        assert_eq!(command, "vim");
        assert!(args.is_empty());
    }
}
