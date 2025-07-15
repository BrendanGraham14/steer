use std::collections::HashMap;
use std::sync::Arc;
use steer_core::auth::ProviderRegistry;
use steer_core::config::provider::{ProviderConfig, ProviderId};

#[derive(Debug, Clone)]
pub struct SetupState {
    pub current_step: SetupStep,
    pub auth_providers: HashMap<ProviderId, AuthStatus>,
    pub selected_provider: Option<ProviderId>,
    pub oauth_state: Option<OAuthFlowState>,
    pub api_key_input: String,
    pub oauth_callback_input: String,
    pub error_message: Option<String>,
    pub provider_cursor: usize,
    pub skip_setup: bool,
    pub registry: Arc<ProviderRegistry>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetupStep {
    Welcome,
    ProviderSelection,
    Authentication(ProviderId),
    Completion,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthStatus {
    NotConfigured,
    ApiKeySet,
    OAuthConfigured,
    InProgress,
}

#[derive(Debug, Clone)]
pub struct OAuthFlowState {
    pub auth_url: String,
    pub state: String,
    pub waiting_for_callback: bool,
}

impl SetupState {
    pub fn new(
        registry: Arc<ProviderRegistry>,
        auth_providers: HashMap<ProviderId, AuthStatus>,
    ) -> Self {
        Self {
            current_step: SetupStep::Welcome,
            auth_providers,
            selected_provider: None,
            oauth_state: None,
            api_key_input: String::new(),
            oauth_callback_input: String::new(),
            error_message: None,
            provider_cursor: 0,
            skip_setup: false,
            registry,
        }
    }

    /// Create a SetupState that skips the welcome page - for /auth command
    pub fn new_for_auth_command(
        registry: Arc<ProviderRegistry>,
        auth_providers: HashMap<ProviderId, AuthStatus>,
    ) -> Self {
        let mut state = Self::new(registry, auth_providers);
        state.current_step = SetupStep::ProviderSelection;
        state
    }

    pub fn next_step(&mut self) {
        self.current_step = match &self.current_step {
            SetupStep::Welcome => SetupStep::ProviderSelection,
            SetupStep::ProviderSelection => {
                if let Some(provider) = &self.selected_provider {
                    SetupStep::Authentication(provider.clone())
                } else {
                    SetupStep::ProviderSelection
                }
            }
            SetupStep::Authentication(_) => SetupStep::Completion,
            SetupStep::Completion => SetupStep::Completion,
        };
        self.error_message = None;
    }

    pub fn previous_step(&mut self) {
        self.current_step = match &self.current_step {
            SetupStep::Welcome => SetupStep::Welcome,
            SetupStep::ProviderSelection => SetupStep::Welcome,
            SetupStep::Authentication(_) => SetupStep::ProviderSelection,
            SetupStep::Completion => SetupStep::ProviderSelection,
        };
        self.error_message = None;
    }

    pub fn available_providers(&self) -> Vec<&ProviderConfig> {
        let mut providers: Vec<_> = self.registry.all().collect();
        // Sort by name for consistent ordering
        providers.sort_by_key(|p| p.name.clone());
        providers
    }
}
