use std::collections::HashMap;
use std::sync::Arc;
use steer_core::config::provider::ProviderId;

#[derive(Debug, Clone)]
pub struct SetupState {
    pub current_step: SetupStep,
    pub auth_providers: HashMap<ProviderId, AuthStatus>,
    pub selected_provider: Option<ProviderId>,
    pub auth_flow_id: Option<String>,
    pub auth_progress: Option<steer_grpc::proto::AuthProgress>,
    pub auth_input: String,
    pub error_message: Option<String>,
    pub provider_cursor: usize,
    pub skip_setup: bool,
    pub registry: Arc<RemoteProviderRegistry>,
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

/// Minimal provider view built from remote proto ProviderInfo
#[derive(Debug, Clone)]
pub struct RemoteProviderConfig {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct RemoteProviderRegistry {
    providers: Vec<RemoteProviderConfig>,
}

impl RemoteProviderRegistry {
    pub fn from_proto(providers: Vec<steer_grpc::proto::ProviderInfo>) -> Self {
        let providers = providers
            .into_iter()
            .map(|p| RemoteProviderConfig {
                id: p.id,
                name: p.name,
            })
            .collect();
        Self { providers }
    }

    pub fn all(&self) -> impl Iterator<Item = &RemoteProviderConfig> {
        self.providers.iter()
    }

    pub fn get(&self, id: &ProviderId) -> Option<&RemoteProviderConfig> {
        self.providers.iter().find(|p| p.id == id.storage_key())
    }
}

impl SetupState {
    pub fn new(
        registry: Arc<RemoteProviderRegistry>,
        auth_providers: HashMap<ProviderId, AuthStatus>,
    ) -> Self {
        Self {
            current_step: SetupStep::Welcome,
            auth_providers,
            selected_provider: None,
            auth_flow_id: None,
            auth_progress: None,
            auth_input: String::new(),
            error_message: None,
            provider_cursor: 0,
            skip_setup: false,
            registry,
        }
    }

    /// Create a SetupState that skips the welcome page - for /auth command
    pub fn new_for_auth_command(
        registry: Arc<RemoteProviderRegistry>,
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

    pub fn available_providers(&self) -> Vec<&RemoteProviderConfig> {
        let mut providers: Vec<_> = self.registry.all().collect();
        // Sort by name for consistent ordering
        providers.sort_by_key(|p| p.name.clone());
        providers
    }
}
