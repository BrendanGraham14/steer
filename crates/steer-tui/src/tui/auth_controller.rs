use std::sync::Arc;
use steer_core::auth::DynAuthenticationFlow;

/// Controller for managing authentication flow state
pub struct AuthController {
    pub flow: Arc<dyn DynAuthenticationFlow>,
    pub state: Box<dyn std::any::Any + Send + Sync>,
}

impl AuthController {
    #[allow(dead_code)]
    pub fn new(
        flow: Arc<dyn DynAuthenticationFlow>,
        state: Box<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self { flow, state }
    }
}
