pub mod anthropic;
pub mod api_key;
pub mod callback_server;
pub mod error;
pub mod openai;
pub mod plugin_registry;
pub mod registry;
pub mod storage;

use std::sync::Arc;

pub use error::{AuthError, Result};
pub use plugin_registry::AuthPluginRegistry;
pub use registry::ProviderRegistry;
pub use storage::{AuthStorage, AuthTokens, Credential, CredentialType, DefaultAuthStorage};
pub use steer_auth_plugin::flow::{
    AuthFlowWrapper, AuthMethod, AuthProgress, AuthenticationFlow, DynAuthenticationFlow,
};
pub use steer_auth_plugin::identifiers::{ModelId, ProviderId};
pub use steer_auth_plugin::{AnthropicAuth, AuthPlugin, OpenAiResponsesAuth};
pub use steer_auth_plugin::{
    ApiKeyOrigin, AuthDirective, AuthErrorAction, AuthErrorContext, AuthHeaderContext,
    AuthHeaderProvider, AuthSource, HeaderPair, InstructionPolicy, ModelVisibilityPolicy,
    RequestKind,
};

/// Marker trait for providers that support interactive authentication
pub trait InteractiveAuth: Send + Sync {
    /// Create an authentication flow for interactive setup
    fn create_auth_flow(
        &self,
        storage: Arc<dyn AuthStorage>,
    ) -> Option<Box<dyn DynAuthenticationFlow>>;
}
