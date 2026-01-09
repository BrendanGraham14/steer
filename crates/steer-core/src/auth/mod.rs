pub mod anthropic;
pub mod api_key;
pub mod callback_server;
pub mod error;
pub mod openai;
pub mod registry;
pub mod storage;

use async_trait::async_trait;
use std::sync::Arc;

pub use error::{AuthError, Result};
pub use registry::ProviderRegistry;
pub use storage::{AuthStorage, AuthTokens, Credential, CredentialType, DefaultAuthStorage};

/// Available authentication methods for a provider
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    OAuth,
    ApiKey,
}

/// Progress status for authentication flows
#[derive(Debug, Clone)]
pub enum AuthProgress {
    /// Need input from the user with a prompt message
    NeedInput(String),
    /// Authentication is in progress with a status message
    InProgress(String),
    /// Authentication is complete
    Complete,
    /// An error occurred
    Error(String),
    /// OAuth flow started, contains the authorization URL
    OAuthStarted { auth_url: String },
}

/// Generic authentication flow trait that providers can implement
#[async_trait]
pub trait AuthenticationFlow: Send + Sync {
    /// The state type for this authentication flow
    type State: Send + Sync;

    /// Get available authentication methods for this provider
    fn available_methods(&self) -> Vec<AuthMethod>;

    /// Start an authentication flow
    async fn start_auth(&self, method: AuthMethod) -> Result<Self::State>;

    /// Get initial progress/instructions after starting auth
    async fn get_initial_progress(
        &self,
        state: &Self::State,
        method: AuthMethod,
    ) -> Result<AuthProgress>;

    /// Handle user input during authentication
    async fn handle_input(&self, state: &mut Self::State, input: &str) -> Result<AuthProgress>;

    /// Check if the provider is already authenticated
    async fn is_authenticated(&self) -> Result<bool>;

    /// Get a display name for the provider
    fn provider_name(&self) -> String;
}

/// Type-erased authentication flow for dynamic dispatch
#[async_trait]
pub trait DynAuthenticationFlow: Send + Sync {
    /// Get available authentication methods for this provider
    fn available_methods(&self) -> Vec<AuthMethod>;

    /// Start an authentication flow
    async fn start_auth(&self, method: AuthMethod) -> Result<Box<dyn std::any::Any + Send + Sync>>;

    /// Get initial progress/instructions after starting auth
    async fn get_initial_progress(
        &self,
        state: &Box<dyn std::any::Any + Send + Sync>,
        method: AuthMethod,
    ) -> Result<AuthProgress>;

    /// Handle user input during authentication
    async fn handle_input(
        &self,
        state: &mut Box<dyn std::any::Any + Send + Sync>,
        input: &str,
    ) -> Result<AuthProgress>;

    /// Check if the provider is already authenticated
    async fn is_authenticated(&self) -> Result<bool>;

    /// Get a display name for the provider
    fn provider_name(&self) -> String;
}

/// Wrapper to convert a concrete AuthenticationFlow into a DynAuthenticationFlow
pub struct AuthFlowWrapper<T: AuthenticationFlow> {
    inner: T,
}

impl<T: AuthenticationFlow> AuthFlowWrapper<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<T: AuthenticationFlow + 'static> DynAuthenticationFlow for AuthFlowWrapper<T>
where
    T::State: 'static,
{
    fn available_methods(&self) -> Vec<AuthMethod> {
        self.inner.available_methods()
    }

    async fn start_auth(&self, method: AuthMethod) -> Result<Box<dyn std::any::Any + Send + Sync>> {
        let state = self.inner.start_auth(method).await?;
        Ok(Box::new(state))
    }

    async fn get_initial_progress(
        &self,
        state: &Box<dyn std::any::Any + Send + Sync>,
        method: AuthMethod,
    ) -> Result<AuthProgress> {
        let concrete_state = state
            .downcast_ref::<T::State>()
            .ok_or_else(|| AuthError::InvalidResponse("Invalid state type".to_string()))?;
        self.inner
            .get_initial_progress(concrete_state, method)
            .await
    }

    async fn handle_input(
        &self,
        state: &mut Box<dyn std::any::Any + Send + Sync>,
        input: &str,
    ) -> Result<AuthProgress> {
        let concrete_state = state
            .downcast_mut::<T::State>()
            .ok_or_else(|| AuthError::InvalidResponse("Invalid state type".to_string()))?;
        self.inner.handle_input(concrete_state, input).await
    }

    async fn is_authenticated(&self) -> Result<bool> {
        self.inner.is_authenticated().await
    }

    fn provider_name(&self) -> String {
        self.inner.provider_name()
    }
}

/// Marker trait for providers that support interactive authentication
pub trait InteractiveAuth: Send + Sync {
    /// Create an authentication flow for interactive setup
    fn create_auth_flow(
        &self,
        storage: Arc<dyn AuthStorage>,
    ) -> Option<Box<dyn DynAuthenticationFlow>>;
}
