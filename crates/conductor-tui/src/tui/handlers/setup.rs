use crate::error::Result;
use crate::tui::InputMode;
use crate::tui::Tui;
use crate::tui::auth_controller::AuthController;
use crate::tui::state::{AuthStatus, SetupState, SetupStep};
use conductor_core::api::ProviderKind;
use conductor_core::auth::{AuthMethod, AuthProgress, DefaultAuthStorage, ProviderRegistry};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::Arc;
use tracing::debug;

// Remove TODO comment - authentication is now handled using the generic trait
pub struct SetupHandler;

impl SetupHandler {
    /// Ensure an auth controller exists for the given provider and method
    async fn ensure_controller(
        tui: &mut Tui,
        provider: ProviderKind,
        method: AuthMethod,
    ) -> Result<()> {
        if tui.auth_controller.is_some() {
            return Ok(());
        }

        let auth_storage = Arc::new(
            DefaultAuthStorage::new().map_err(|e| crate::error::Error::Auth(e.to_string()))?,
        );

        if let Some(auth_flow) = ProviderRegistry::create_auth_flow(provider, auth_storage) {
            match auth_flow.start_auth(method).await {
                Ok(auth_state) => {
                    tui.auth_controller = Some(AuthController {
                        flow: Arc::from(auth_flow),
                        state: auth_state,
                    });
                    // Mark authentication as in progress
                    if let Some(setup_state) = &mut tui.setup_state {
                        setup_state
                            .auth_providers
                            .insert(provider, AuthStatus::InProgress);
                    }
                }
                Err(e) => {
                    if let Some(setup_state) = &mut tui.setup_state {
                        setup_state.error_message =
                            Some(format!("Failed to initialize authentication: {e}"));
                    }
                    return Err(crate::error::Error::Auth(e.to_string()));
                }
            }
        } else if let Some(setup_state) = &mut tui.setup_state {
            setup_state.error_message = Some(format!("{provider} doesn't support authentication"));
        }

        Ok(())
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
            SetupStep::Authentication(provider) => {
                let provider = *provider;
                Self::handle_authentication(tui, provider, key).await
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
                if let Some(provider) = providers.get(state.provider_cursor) {
                    state.selected_provider = Some(*provider);
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
        provider: ProviderKind,
        key: KeyEvent,
    ) -> Result<Option<InputMode>> {
        // For non-Anthropic providers, automatically initialize API key auth
        if provider != ProviderKind::Anthropic
            && tui.auth_controller.is_none()
            && Self::ensure_controller(tui, provider, AuthMethod::ApiKey)
                .await
                .is_err()
        {
            // Error message already set in ensure_controller
            return Ok(None);
        }

        match key.code {
            KeyCode::Char('1') if provider == ProviderKind::Anthropic => {
                // Start OAuth flow
                if Self::ensure_controller(tui, provider, AuthMethod::OAuth)
                    .await
                    .is_err()
                {
                    return Ok(None);
                }

                // Get the auth controller we just created
                if let Some(ref auth_controller) = tui.auth_controller {
                    // Get initial progress to extract auth URL
                    match auth_controller
                        .flow
                        .get_initial_progress(&auth_controller.state, AuthMethod::OAuth)
                        .await
                    {
                        Ok(AuthProgress::OAuthStarted { auth_url }) => {
                            let state = tui.setup_state.as_mut().unwrap();
                            state.oauth_state = Some(crate::tui::state::OAuthFlowState {
                                auth_url: auth_url.clone(),
                                state: String::new(),
                                waiting_for_callback: true,
                            });

                            // Try to open browser
                            if let Err(e) = open::that(&auth_url) {
                                state.error_message = Some(format!("Failed to open browser: {e}"));
                            }
                        }
                        _ => {
                            let state = tui.setup_state.as_mut().unwrap();
                            state.error_message = Some("Failed to get OAuth URL".to_string());
                        }
                    }
                }
                Ok(None)
            }
            KeyCode::Char('2') if provider == ProviderKind::Anthropic => {
                // Check conditions before calling ensure_controller
                let should_proceed = {
                    let state = tui.setup_state.as_ref().unwrap();
                    state.api_key_input.is_empty() && state.oauth_state.is_none()
                };

                if should_proceed {
                    // API key input mode
                    debug!("Starting API key input mode");
                    let state = tui.setup_state.as_mut().unwrap();
                    state.api_key_input.clear();
                    if Self::ensure_controller(tui, provider, AuthMethod::ApiKey)
                        .await
                        .is_err()
                    {
                        return Ok(None);
                    }
                }
                Ok(None)
            }
            KeyCode::Enter => {
                // Check what type of input we have
                let has_oauth_callback = {
                    let state = tui.setup_state.as_ref().unwrap();
                    state.oauth_state.is_some() && !state.oauth_callback_input.is_empty()
                };

                let has_api_key = {
                    let state = tui.setup_state.as_ref().unwrap();
                    !state.api_key_input.is_empty()
                };

                if has_oauth_callback {
                    // Handle OAuth callback
                    let state = tui.setup_state.as_mut().unwrap();
                    if let Some(ref mut auth_controller) = tui.auth_controller {
                        let input = state.oauth_callback_input.clone();

                        match auth_controller
                            .flow
                            .handle_input(&mut auth_controller.state, &input)
                            .await
                        {
                            Ok(AuthProgress::Complete) => {
                                state.oauth_state = None;
                                state.oauth_callback_input.clear();
                                state
                                    .auth_providers
                                    .insert(provider, AuthStatus::OAuthConfigured);
                                state.error_message =
                                    Some("OAuth authentication successful!".to_string());
                                // Clear auth controller for next provider
                                tui.auth_controller = None;
                                // Return to provider selection to allow authenticating with other providers
                                state.current_step = SetupStep::ProviderSelection;
                                state.selected_provider = None;
                            }
                            Ok(AuthProgress::NeedInput(prompt)) => {
                                state.error_message = Some(prompt);
                            }
                            Ok(AuthProgress::InProgress(msg)) => {
                                state.error_message = Some(msg);
                            }
                            Ok(AuthProgress::Error(err)) => {
                                state.error_message = Some(err);
                                state.oauth_callback_input.clear();
                            }
                            Ok(AuthProgress::OAuthStarted { .. }) => {
                                // Shouldn't happen at this stage
                                state.error_message = Some("Unexpected OAuth state".to_string());
                            }
                            Err(e) => {
                                state.error_message =
                                    Some(format!("OAuth authentication failed: {e}"));
                                state.oauth_callback_input.clear();
                            }
                        }
                    }
                } else if has_api_key {
                    // Ensure we have a controller for API key auth
                    if Self::ensure_controller(tui, provider, AuthMethod::ApiKey)
                        .await
                        .is_err()
                    {
                        return Ok(None);
                    }

                    // Handle API key input
                    let state = tui.setup_state.as_mut().unwrap();
                    if let Some(ref mut auth_controller) = tui.auth_controller {
                        let api_key = state.api_key_input.clone();

                        match auth_controller
                            .flow
                            .handle_input(&mut auth_controller.state, &api_key)
                            .await
                        {
                            Ok(AuthProgress::Complete) => {
                                state.api_key_input.clear();
                                state.auth_providers.insert(provider, AuthStatus::ApiKeySet);
                                state.error_message =
                                    Some(format!("API key successfully imported for {provider}!"));
                                // Clear auth controller for next provider
                                tui.auth_controller = None;
                                // Return to provider selection to allow authenticating with other providers
                                state.current_step = SetupStep::ProviderSelection;
                                state.selected_provider = None;
                            }
                            Ok(AuthProgress::Error(err)) => {
                                state.error_message = Some(err);
                                state.api_key_input.clear();
                            }
                            Err(e) => {
                                state.error_message = Some(e.to_string());
                                state.api_key_input.clear();
                            }
                            _ => {}
                        }
                    }
                }
                Ok(None)
            }
            KeyCode::Char(c) => {
                let state = tui.setup_state.as_mut().unwrap();
                if state.oauth_state.is_some() {
                    // Typing OAuth callback
                    state.oauth_callback_input.push(c);
                    Ok(None)
                } else if tui.auth_controller.is_some()
                    && (provider != ProviderKind::Anthropic
                        || (provider == ProviderKind::Anthropic && state.oauth_state.is_none()))
                {
                    // Typing API key
                    state.api_key_input.push(c);
                    Ok(None)
                } else {
                    Ok(None)
                }
            }
            KeyCode::Backspace => {
                let state = tui.setup_state.as_mut().unwrap();
                if state.oauth_state.is_some() {
                    state.oauth_callback_input.pop();
                    Ok(None)
                } else if tui.auth_controller.is_some()
                    && (provider != ProviderKind::Anthropic
                        || (provider == ProviderKind::Anthropic && state.oauth_state.is_none()))
                {
                    state.api_key_input.pop();
                    Ok(None)
                } else {
                    Ok(None)
                }
            }
            KeyCode::Esc => {
                let state = tui.setup_state.as_mut().unwrap();
                state.oauth_state = None;
                state.api_key_input.clear();
                state.oauth_callback_input.clear();
                state.error_message = None; // Clear any error messages
                // Reset auth status if it was in progress
                if let Some(provider) = state.selected_provider {
                    if state.auth_providers.get(&provider) == Some(&AuthStatus::InProgress) {
                        state
                            .auth_providers
                            .insert(provider, AuthStatus::NotConfigured);
                    }
                }
                tui.auth_controller = None; // Clear auth controller
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
                    let prefs =
                        conductor_core::preferences::Preferences::load().unwrap_or_default();

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
