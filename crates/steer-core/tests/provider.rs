use steer_core::config::provider::{Provider, builtin_providers};

#[test]
fn parses_default_providers() {
    let providers = builtin_providers().expect("parse embedded TOML");
    assert_eq!(providers.len(), 4);

    let ids: Vec<_> = providers.iter().map(|p| &p.id).collect::<Vec<_>>();
    use Provider as P;
    assert!(ids.iter().any(|id| matches!(id, P::Anthropic)));
    assert!(ids.iter().any(|id| matches!(id, P::Openai)));
    assert!(ids.iter().any(|id| matches!(id, P::Google)));
    assert!(ids.iter().any(|id| matches!(id, P::Xai)));

    let anthro = providers
        .iter()
        .find(|p| matches!(p.id, P::Anthropic))
        .unwrap();
    assert_eq!(anthro.auth_schemes.len(), 2);
}
