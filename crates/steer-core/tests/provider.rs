use steer_core::auth::ProviderRegistry;
use steer_core::config::provider;

#[test]
fn parses_default_providers() {
    let registry = ProviderRegistry::load(&[]).expect("load provider registry");
    let providers: Vec<_> = registry.all().cloned().collect();
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
