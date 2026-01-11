use crate::error::Result;
use crate::tui::InputMode;
use crate::tui::Tui;
use crate::tui::state::{AuthStatus, SetupState, SetupStep};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use steer_core::config::provider::ProviderId;
use tracing::debug;

// Remove TODO comment - authentication is now handled using the generic trait
pub struct SetupHandler;

impl SetupHandler {
    async fn start_auth_flow(tui: &mut Tui, provider_id: &ProviderId) -> Result<()> {
        let response = tui
            .client
            .start_auth(provider_id.storage_key())
            .await
            .map_err(|e| crate::error::Error::Auth(e.to_string()))?;

        let setup_state = tui.setup_state.as_mut().ok_or_else(|| {
            crate::error::Error::Generic("Missing setup state during auth".to_string())
        })?;

        setup_state.auth_flow_id = Some(response.flow_id.clone());
        setup_state.auth_progress = response.progress;
        setup_state.auth_input.clear();
        setup_state
            .auth_providers
            .insert(provider_id.clone(), AuthStatus::InProgress);

        if let Some(progress) = setup_state.auth_progress.as_ref() {
            if let Some(steer_grpc::proto::auth_progress::State::OauthStarted(oauth)) =
                progress.state.as_ref()
            {
                if let Err(e) = open::that(&oauth.auth_url) {
                    setup_state.error_message = Some(format!("Failed to open browser: {e}"));
                }
            }
        }

        Ok(())
    }

    async fn refresh_auth_status(tui: &mut Tui, provider_id: &ProviderId) -> Result<AuthStatus> {
        let statuses = tui
            .client
            .get_provider_auth_status(Some(provider_id.storage_key()))
            .await
            .map_err(|e| crate::error::Error::Auth(e.to_string()))?;
        let status = statuses
            .first()
            .and_then(|s| s.auth_source.as_ref())
            .and_then(|source| source.source.as_ref());

        let mapped = match status {
            Some(steer_grpc::proto::auth_source::Source::ApiKey(_)) => AuthStatus::ApiKeySet,
            Some(steer_grpc::proto::auth_source::Source::Plugin(_)) => AuthStatus::OAuthConfigured,
            Some(steer_grpc::proto::auth_source::Source::None(_)) | None => {
                AuthStatus::NotConfigured
            }
        };

        Ok(mapped)
    }

    pub async fn handle_key_event(tui: &mut Tui, key: KeyEvent) -> Result<Option<InputMode>> {
        // Clone the current step to avoid borrow conflicts
        let current_step = tui.setup_state.as_ref().unwrap().current_step.clone();

        debug!(
            "SetupHandler::handle_key_event - step: {:?}, key: {:?}",
            current_step, key
        );

        match &current_step {
            SetupStep::Welcome => Self::handle_welcome(tui.setup_state.as_mut().unwrap(), key),
            SetupStep::ProviderSelection => {
                Self::handle_provider_selection(tui.setup_state.as_mut().unwrap(), key)
            }
            SetupStep::Authentication(provider_id) => {
                let provider_id = provider_id.clone();
                Self::handle_authentication(tui, provider_id, key).await
            }
            SetupStep::Completion => Self::handle_completion(tui, key).await,
        }
    }

    fn handle_welcome(state: &mut SetupState, key: KeyEvent) -> Result<Option<InputMode>> {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => {
                state.next_step();
                Ok(None)
            }
            (KeyCode::Char('s'), KeyModifiers::NONE)
            | (KeyCode::Char('S'), KeyModifiers::NONE)
            | (KeyCode::Esc, KeyModifiers::NONE)
            | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                state.skip_setup = true;
                Ok(Some(InputMode::Simple))
            }
            _ => Ok(None),
        }
    }

    fn handle_provider_selection(
        state: &mut SetupState,
        key: KeyEvent,
    ) -> Result<Option<InputMode>> {
        debug!("handle_provider_selection: key={:?}", key);

        let providers = state.available_providers();
        if providers.is_empty() {
            debug!("No providers available");
            return Ok(None);
        }

        debug!(
            "Current cursor: {}, providers: {:?}",
            state.provider_cursor, providers
        );

        match (key.code, key.modifiers) {
            (KeyCode::Up, KeyModifiers::NONE) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                debug!("Up/k pressed");
                if state.provider_cursor > 0 {
                    state.provider_cursor -= 1;
                } else {
                    state.provider_cursor = providers.len() - 1;
                }
                debug!("New cursor: {}", state.provider_cursor);
                Ok(None)
            }
            (KeyCode::Down, KeyModifiers::NONE) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                debug!("Down/j pressed");
                state.provider_cursor = (state.provider_cursor + 1) % providers.len();
                debug!("New cursor: {}", state.provider_cursor);
                Ok(None)
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if let Some(provider_config) = providers.get(state.provider_cursor) {
                    state.selected_provider = Some(steer_core::config::provider::ProviderId(
                        provider_config.id.clone(),
                    ));
                    state.next_step();
                }
                Ok(None)
            }
            (KeyCode::Char('s'), KeyModifiers::NONE) | (KeyCode::Char('S'), KeyModifiers::NONE) => {
                state.skip_setup = true;
                Ok(Some(InputMode::Simple))
            }
            (KeyCode::Esc, KeyModifiers::NONE) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                // If we're at provider selection and this is from /auth command,
                // exit setup mode instead of going to welcome
                if matches!(state.current_step, SetupStep::ProviderSelection) {
                    // Clear setup state and return to normal mode
                    Ok(Some(InputMode::Simple))
                } else {
                    state.previous_step();
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    async fn handle_authentication(
        tui: &mut Tui,
        provider_id: ProviderId,
        key: KeyEvent,
    ) -> Result<Option<InputMode>> {
        if tui
            .setup_state
            .as_ref()
            .and_then(|s| s.auth_flow_id.as_ref())
            .is_none()
        {
            Self::start_auth_flow(tui, &provider_id).await?;
        }

        match key.code {
            KeyCode::Enter => {
                let progress_state = {
                    let setup_state = tui.setup_state.as_ref().unwrap();
                    setup_state
                        .auth_progress
                        .as_ref()
                        .and_then(|progress| progress.state.as_ref())
                        .cloned()
                };

                let expects_input = matches!(
                    progress_state,
                    Some(steer_grpc::proto::auth_progress::State::NeedInput(_))
                        | Some(steer_grpc::proto::auth_progress::State::OauthStarted(_))
                );

                let flow_id = tui
                    .setup_state
                    .as_ref()
                    .and_then(|s| s.auth_flow_id.clone());

                if expects_input
                    && let (Some(flow_id), Some(input)) = (
                        flow_id,
                        tui.setup_state
                            .as_ref()
                            .map(|s| s.auth_input.clone())
                            .filter(|input| !input.is_empty()),
                    ) {
                        let progress = tui
                            .client
                            .send_auth_input(flow_id.clone(), input)
                            .await
                            .map_err(|e| crate::error::Error::Auth(e.to_string()))?;

                    let mut completed = false;
                    let mut error_message = None;

                        match &progress.state {
                            Some(steer_grpc::proto::auth_progress::State::Complete(_)) => {
                                completed = true;
                            }
                            Some(steer_grpc::proto::auth_progress::State::Error(error)) => {
                                error_message = Some(error.message.clone());
                            }
                            _ => {}
                        }

                        {
                            let setup_state = tui.setup_state.as_mut().unwrap();
                            setup_state.auth_progress = Some(progress.clone());
                            setup_state.auth_input.clear();

                            if let Some(message) = &error_message {
                                setup_state.error_message = Some(message.clone());
                                setup_state.auth_flow_id = None;
                                setup_state.auth_progress = None;
                            }
                        }

                        if completed {
                            let status = Self::refresh_auth_status(tui, &provider_id).await?;
                            let setup_state = tui.setup_state.as_mut().unwrap();
                            setup_state
                                .auth_providers
                                .insert(provider_id.clone(), status);
                            setup_state.error_message =
                                Some("Authentication successful!".to_string());
                            setup_state.auth_flow_id = None;
                            setup_state.auth_progress = None;
                        }
                    }
                Ok(None)
            }
            KeyCode::Char(c) => {
                let state = tui.setup_state.as_mut().unwrap();
                let expects_input = state
                    .auth_progress
                    .as_ref()
                    .and_then(|progress| progress.state.as_ref())
                    .map(|state| {
                        matches!(
                            state,
                            steer_grpc::proto::auth_progress::State::NeedInput(_)
                                | steer_grpc::proto::auth_progress::State::OauthStarted(_)
                        )
                    })
                    .unwrap_or(false);
                if expects_input {
                    state.auth_input.push(c);
                }
                Ok(None)
            }
            KeyCode::Backspace => {
                let state = tui.setup_state.as_mut().unwrap();
                let expects_input = state
                    .auth_progress
                    .as_ref()
                    .and_then(|progress| progress.state.as_ref())
                    .map(|state| {
                        matches!(
                            state,
                            steer_grpc::proto::auth_progress::State::NeedInput(_)
                                | steer_grpc::proto::auth_progress::State::OauthStarted(_)
                        )
                    })
                    .unwrap_or(false);
                if expects_input {
                    state.auth_input.pop();
                }
                Ok(None)
            }
            KeyCode::Esc => {
                let state = tui.setup_state.as_mut().unwrap();
                if let Some(flow_id) = state.auth_flow_id.take() {
                    let _ = tui.client.cancel_auth(flow_id).await;
                }
                state.auth_progress = None;
                state.auth_input.clear();
                state.error_message = None; // Clear any error messages
                // Reset auth status if it was in progress
                if let Some(provider) = &state.selected_provider {
                    if state.auth_providers.get(provider) == Some(&AuthStatus::InProgress) {
                        state
                            .auth_providers
                            .insert(provider.clone(), AuthStatus::NotConfigured);
                    }
                }
                state.previous_step();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn handle_completion(tui: &mut Tui, key: KeyEvent) -> Result<Option<InputMode>> {
        match key.code {
            KeyCode::Enter => {
                // Save preferences
                if let Some(_setup_state) = &tui.setup_state {
                    // Load existing preferences or use defaults
                    let prefs = steer_core::preferences::Preferences::load().unwrap_or_default();

                    // Model selection has been removed from setup flow
                    // Just save the existing preferences without modification

                    // Save the preferences
                    prefs
                        .save()
                        .map_err(|e| crate::error::Error::Config(e.to_string()))?;
                }

                // Clear setup state and transition to normal mode
                tui.setup_state = None;
                Ok(Some(InputMode::Simple))
            }
            _ => Ok(None),
        }
    }
}

impl SetupHandler {
    pub async fn poll_oauth_callback(tui: &mut Tui) -> Result<bool> {
        let provider_id = {
            let Some(setup_state) = tui.setup_state.as_ref() else {
                return Ok(false);
            };
            match &setup_state.current_step {
                SetupStep::Authentication(provider_id) => provider_id.clone(),
                _ => return Ok(false),
            }
        };

        let flow_id = tui
            .setup_state
            .as_ref()
            .and_then(|state| state.auth_flow_id.clone());

        if flow_id.is_none() {
            Self::start_auth_flow(tui, &provider_id).await?;
            return Ok(true);
        }

        let flow_id = flow_id.expect("auth_flow_id just checked");

        if tui
            .setup_state
            .as_ref()
            .map(|state| !state.auth_input.is_empty())
            .unwrap_or(false)
        {
            return Ok(false);
        }

        let should_poll = tui
            .setup_state
            .as_ref()
            .and_then(|state| state.auth_progress.as_ref())
            .and_then(|progress| progress.state.as_ref())
            .map(|state| {
                matches!(
                    state,
                    steer_grpc::proto::auth_progress::State::OauthStarted(_)
                        | steer_grpc::proto::auth_progress::State::InProgress(_)
                )
            })
            .unwrap_or(false);

        if !should_poll {
            return Ok(false);
        }

        let progress = tui
            .client
            .get_auth_progress(flow_id.clone())
            .await
            .map_err(|e| crate::error::Error::Auth(e.to_string()))?;

        let mut completed = false;
        let mut error_message = None;

        match &progress.state {
            Some(steer_grpc::proto::auth_progress::State::Complete(_)) => {
                completed = true;
            }
            Some(steer_grpc::proto::auth_progress::State::Error(error)) => {
                error_message = Some(error.message.clone());
            }
            _ => {}
        }

        {
            let Some(setup_state) = tui.setup_state.as_mut() else {
                return Ok(false);
            };
            setup_state.auth_progress = Some(progress.clone());

            if let Some(message) = &error_message {
                setup_state.error_message = Some(message.clone());
                setup_state.auth_flow_id = None;
                setup_state.auth_progress = None;
            }
        }

        if completed {
            let status = Self::refresh_auth_status(tui, &provider_id).await?;
            let Some(setup_state) = tui.setup_state.as_mut() else {
                return Ok(false);
            };
            setup_state
                .auth_providers
                .insert(provider_id.clone(), status);
            setup_state.error_message = Some("Authentication successful!".to_string());
            setup_state.auth_flow_id = None;
            setup_state.auth_progress = None;
            setup_state.current_step = SetupStep::ProviderSelection;
            setup_state.selected_provider = None;
            return Ok(true);
        }

        if error_message.is_some() {
            return Ok(true);
        }

        Ok(false)
    }
}
