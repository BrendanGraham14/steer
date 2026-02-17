use std::cmp::Ordering;

use steer_core::config::model::{ModelConfig, ModelId};
use steer_core::model_registry::ModelRegistry;
use tracing::{debug, warn};

pub struct ModelSelection {
    pub default_model: ModelId,
    pub preferred_model: Option<ModelId>,
}

pub fn resolve_model_selection(
    preferred_model: Option<&str>,
    catalog_paths: &[String],
) -> ModelSelection {
    let builtin_default = steer_core::config::model::builtin::default_model();

    let registry = match ModelRegistry::load(catalog_paths) {
        Ok(registry) => registry,
        Err(e) => {
            warn!(
                "Failed to load model registry for catalogs {:?}: {}. Falling back to builtin default.",
                catalog_paths, e
            );
            return ModelSelection {
                default_model: builtin_default,
                preferred_model: None,
            };
        }
    };

    let preferred_model = if let Some(model_str) = preferred_model {
        match registry.resolve(model_str) {
            Ok(id) => Some(id),
            Err(e) => {
                warn!(
                    "Failed to resolve preferred model '{}': {}. Falling back to recommended/default.",
                    model_str, e
                );
                None
            }
        }
    } else {
        None
    };

    let default_model = preferred_model
        .clone()
        .unwrap_or_else(|| select_recommended_model(&registry, &builtin_default));

    ModelSelection {
        default_model,
        preferred_model,
    }
}

fn select_recommended_model(registry: &ModelRegistry, builtin_default: &ModelId) -> ModelId {
    if let Some(config) = registry.get(builtin_default)
        && config.recommended
    {
        return builtin_default.clone();
    }

    let mut recommended: Vec<&ModelConfig> = registry.recommended().collect();
    if recommended.is_empty() {
        return builtin_default.clone();
    }

    recommended.sort_by(|a, b| {
        let provider_cmp = a.provider.as_str().cmp(b.provider.as_str());
        if provider_cmp == Ordering::Equal {
            a.id.cmp(&b.id)
        } else {
            provider_cmp
        }
    });

    let chosen = recommended[0];
    debug!(
        "Selected recommended model {}/{}",
        chosen.provider.as_str(),
        chosen.id
    );

    ModelId::new(chosen.provider.clone(), chosen.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn resolves_preferred_model_when_valid() {
        let builtin_default = steer_core::config::model::builtin::default_model();
        let preferred = builtin_default.to_string();
        let selection = resolve_model_selection(Some(preferred.as_str()), &[]);

        assert_eq!(selection.default_model, builtin_default);
        assert_eq!(selection.preferred_model, Some(builtin_default));
    }

    #[test]
    fn prefers_builtin_default_when_recommended() {
        let builtin_default = steer_core::config::model::builtin::default_model();
        let tmp = TempDir::new().unwrap();
        let catalog_path = tmp.path().join("catalog.toml");
        let catalog = format!(
            r#"
[[providers]]
id = "aaa"
name = "AAA"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "{provider}"
id = "{model_id}"
recommended = true
parameters = {{ max_output_tokens = 4096 }}

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
parameters = {{ max_output_tokens = 4096 }}
"#,
            provider = builtin_default.provider.as_str(),
            model_id = builtin_default.id.as_str()
        );
        fs::write(&catalog_path, catalog).unwrap();

        let selection =
            resolve_model_selection(None, &[catalog_path.to_string_lossy().to_string()]);

        assert_eq!(selection.default_model, builtin_default);
    }

    #[test]
    fn picks_other_recommended_when_builtin_not_recommended() {
        let builtin_default = steer_core::config::model::builtin::default_model();
        let tmp = TempDir::new().unwrap();
        let catalog_path = tmp.path().join("catalog.toml");
        let catalog = format!(
            r#"
[[providers]]
id = "aaa"
name = "AAA"
api_format = "openai-responses"
auth_schemes = ["api-key"]

[[models]]
provider = "{provider}"
id = "{model_id}"
recommended = false
parameters = {{ max_output_tokens = 4096 }}

[[models]]
provider = "aaa"
id = "aaa-model"
recommended = true
parameters = {{ max_output_tokens = 4096 }}
"#,
            provider = builtin_default.provider.as_str(),
            model_id = builtin_default.id.as_str()
        );
        fs::write(&catalog_path, catalog).unwrap();

        let selection =
            resolve_model_selection(None, &[catalog_path.to_string_lossy().to_string()]);

        assert_eq!(
            selection.default_model,
            ModelId::new(
                steer_core::config::provider::ProviderId::from("aaa"),
                "aaa-model"
            )
        );
    }
}
