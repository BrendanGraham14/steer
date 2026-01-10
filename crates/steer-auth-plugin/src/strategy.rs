use crate::flow::AuthMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyOrigin {
    Env,
    Stored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    ApiKey { origin: ApiKeyOrigin },
    Plugin { method: AuthMethod },
    None,
}
