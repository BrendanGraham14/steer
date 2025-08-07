use steer_core::config::provider::{self, builtin_providers};

#[test]
fn parses_default_providers() {
    let providers = builtin_providers().expect("parse embedded TOML");
    assert_eq!(providers.len(), 4);

    let ids: Vec<_> = providers.iter().map(|p| &p.id).collect::<Vec<_>>();
    assert!(ids.iter().any(|id| **id == provider::anthropic()));
    assert!(ids.iter().any(|id| **id == provider::openai()));
    assert!(ids.iter().any(|id| **id == provider::google()));
    assert!(ids.iter().any(|id| **id == provider::xai()));

    let anthro = providers
        .iter()
        .find(|p| p.id == provider::anthropic())
        .unwrap();
    assert_eq!(anthro.auth_schemes.len(), 2);
}
