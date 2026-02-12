use std::{collections::HashMap, path::Path, path::PathBuf, time::Duration};

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Serialize;
use steer_core::preferences::TelemetryPreferences;
use steer_core::utils::paths::AppPaths;
use thiserror::Error;
use uuid::Uuid;

const DEFAULT_TELEMETRY_ENDPOINT: &str = "https://steer-telemetry.fly.dev/v1/events/startup";
const TELEMETRY_ENABLED_ENV: &str = "STEER_TELEMETRY_ENABLED";
const TELEMETRY_ENDPOINT_ENV: &str = "STEER_TELEMETRY_ENDPOINT";
const CI_ENV_VARS: [&str; 9] = [
    "CI",
    "GITHUB_ACTIONS",
    "BUILDKITE",
    "GITLAB_CI",
    "TF_BUILD",
    "JENKINS_URL",
    "TEAMCITY_VERSION",
    "BITBUCKET_BUILD_NUMBER",
    "DRONE",
];

#[derive(Debug, Clone, Copy)]
pub enum StartupCommand {
    Tui,
    Headless,
    Server,
    Unknown,
}

impl StartupCommand {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Headless => "headless",
            Self::Server => "server",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StartupTelemetryContext {
    pub command: StartupCommand,
    pub session_id: Option<Uuid>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Error)]
enum TelemetryError {
    #[error("failed to resolve telemetry install id path")]
    MissingInstallPath,
    #[error("failed to create telemetry directory: {0}")]
    CreateDir(std::io::Error),
    #[error("failed to read telemetry install id: {0}")]
    ReadInstallId(std::io::Error),
    #[error("failed to write telemetry install id: {0}")]
    WriteInstallId(std::io::Error),
    #[error("failed to send telemetry request: {0}")]
    RequestSend(reqwest::Error),
}

#[derive(Debug, Serialize)]
struct StartupEventPayload<'a> {
    event_id: Uuid,
    install_id: &'a str,
    session_id: Option<Uuid>,
    client_timestamp: DateTime<Utc>,
    steer_version: &'static str,
    command: &'a str,
    os: &'static str,
    arch: &'static str,
    provider: Option<&'a str>,
    model: Option<&'a str>,
    is_ci: bool,
    metadata: HashMap<String, String>,
}

pub async fn emit_startup_event(context: StartupTelemetryContext, prefs: TelemetryPreferences) {
    let settings = TelemetrySettings::from_env_and_preferences(prefs);
    if !settings.enabled {
        tracing::debug!(target: "steer::telemetry", "startup telemetry disabled");
        return;
    }

    let install_id = match load_or_create_install_id() {
        Ok(value) => value,
        Err(err) => {
            tracing::debug!(target: "steer::telemetry", error = %err, "skipping startup telemetry: install id unavailable");
            return;
        }
    };

    let payload = StartupEventPayload {
        event_id: Uuid::new_v4(),
        install_id: &install_id,
        session_id: context.session_id,
        client_timestamp: Utc::now(),
        steer_version: env!("CARGO_PKG_VERSION"),
        command: context.command.as_str(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        provider: context.provider.as_deref(),
        model: context.model.as_deref(),
        is_ci: is_ci_environment(),
        metadata: HashMap::new(),
    };

    let client = match Client::builder().timeout(Duration::from_secs(2)).build() {
        Ok(client) => client,
        Err(err) => {
            tracing::debug!(target: "steer::telemetry", error = %err, "skipping startup telemetry: request client unavailable");
            return;
        }
    };

    if let Err(err) = send_payload(client, &settings.endpoint, payload).await {
        tracing::debug!(
            target: "steer::telemetry",
            error = %err,
            endpoint = %settings.endpoint,
            "failed to send startup telemetry"
        );
    }
}

async fn send_payload(
    client: Client,
    endpoint: &str,
    payload: StartupEventPayload<'_>,
) -> Result<(), TelemetryError> {
    let response = client
        .post(endpoint)
        .json(&payload)
        .send()
        .await
        .map_err(TelemetryError::RequestSend)?;

    if !response.status().is_success() {
        tracing::debug!(target: "steer::telemetry", status = %response.status(), "telemetry endpoint returned non-success");
    }

    Ok(())
}

#[derive(Debug)]
struct TelemetrySettings {
    enabled: bool,
    endpoint: String,
}

impl TelemetrySettings {
    fn from_env_and_preferences(preferences: TelemetryPreferences) -> Self {
        let enabled_env = std::env::var(TELEMETRY_ENABLED_ENV).ok();
        let endpoint_env = std::env::var(TELEMETRY_ENDPOINT_ENV).ok();

        Self::from_parts(preferences, enabled_env.as_deref(), endpoint_env.as_deref())
    }

    fn from_parts(
        preferences: TelemetryPreferences,
        enabled_env: Option<&str>,
        endpoint_env: Option<&str>,
    ) -> Self {
        let enabled = enabled_env
            .and_then(parse_enabled)
            .unwrap_or(preferences.enabled);

        let endpoint = endpoint_env
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(preferences.endpoint)
            .unwrap_or_else(|| DEFAULT_TELEMETRY_ENDPOINT.to_string());

        Self { enabled, endpoint }
    }
}

fn parse_enabled(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn telemetry_install_id_path() -> Option<PathBuf> {
    AppPaths::user_data_dir().or_else(|| dirs::home_dir().map(|path| path.join(".steer")))
}

fn load_or_create_install_id() -> Result<String, TelemetryError> {
    let base_dir = telemetry_install_id_path().ok_or(TelemetryError::MissingInstallPath)?;
    std::fs::create_dir_all(&base_dir).map_err(TelemetryError::CreateDir)?;

    let install_id_path = base_dir.join("install_id");

    match std::fs::read_to_string(&install_id_path) {
        Ok(value) => {
            let trimmed = value.trim();
            if validate_install_id(trimmed) {
                Ok(trimmed.to_string())
            } else {
                tracing::debug!(target: "steer::telemetry", "invalid install_id on disk, generating a new id");
                let install_id = Uuid::new_v4().as_simple().to_string();
                persist_install_id(&install_id_path, &install_id)?;
                Ok(install_id)
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let install_id = Uuid::new_v4().as_simple().to_string();
            persist_install_id(&install_id_path, &install_id)?;
            Ok(install_id)
        }
        Err(err) => Err(TelemetryError::ReadInstallId(err)),
    }
}

fn persist_install_id(path: &Path, install_id: &str) -> Result<(), TelemetryError> {
    std::fs::write(path, format!("{install_id}\n")).map_err(TelemetryError::WriteInstallId)
}

fn validate_install_id(value: &str) -> bool {
    let is_valid_len = (8..=128).contains(&value.len());
    let is_hex = value.chars().all(|ch| ch.is_ascii_hexdigit());
    is_valid_len && is_hex
}

fn is_ci_environment() -> bool {
    is_ci_environment_with(|key| std::env::var(key).ok())
}

fn is_ci_environment_with<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    CI_ENV_VARS
        .iter()
        .any(|key| get_env(key).is_some_and(|value| !value.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enabled_accepts_standard_truthy_and_falsy_values() {
        assert_eq!(parse_enabled("true"), Some(true));
        assert_eq!(parse_enabled("1"), Some(true));
        assert_eq!(parse_enabled("yes"), Some(true));
        assert_eq!(parse_enabled("false"), Some(false));
        assert_eq!(parse_enabled("0"), Some(false));
        assert_eq!(parse_enabled("no"), Some(false));
        assert_eq!(parse_enabled("not-a-bool"), None);
    }

    #[test]
    fn validate_install_id_accepts_hex_lengths() {
        assert!(validate_install_id("abcdef12"));
        assert!(validate_install_id(&"a".repeat(128)));
        assert!(!validate_install_id("xyz"));
        assert!(!validate_install_id("abc"));
    }

    #[test]
    fn telemetry_settings_apply_env_overrides() {
        let settings = TelemetrySettings::from_parts(
            TelemetryPreferences {
                enabled: true,
                endpoint: Some("https://prefs.example".to_string()),
            },
            Some("0"),
            Some("https://example.com"),
        );

        assert!(!settings.enabled);
        assert_eq!(settings.endpoint, "https://example.com");
    }

    #[test]
    fn telemetry_settings_fall_back_to_preferences_then_default() {
        let settings = TelemetrySettings::from_parts(
            TelemetryPreferences {
                enabled: false,
                endpoint: None,
            },
            None,
            None,
        );

        assert!(!settings.enabled);
        assert_eq!(settings.endpoint, DEFAULT_TELEMETRY_ENDPOINT);
    }

    #[test]
    fn is_ci_environment_with_detects_non_empty_markers() {
        assert!(is_ci_environment_with(|key| {
            if key == "GITHUB_ACTIONS" {
                Some("true".to_string())
            } else {
                None
            }
        }));

        assert!(!is_ci_environment_with(|_| None));
        assert!(!is_ci_environment_with(|key| {
            if key == "CI" {
                Some("   ".to_string())
            } else {
                None
            }
        }));
    }
}
