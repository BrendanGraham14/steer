pub mod api_key;
pub mod error;
pub mod plugin_registry;
pub mod registry;
pub mod storage;

pub use error::{AuthError, Result};
pub use plugin_registry::AuthPluginRegistry;
pub use registry::ProviderRegistry;
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
pub use storage::{AuthStorage, AuthTokens, Credential, CredentialType, DefaultAuthStorage};
