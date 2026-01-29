use std::env;
use std::fs;
use std::path::Path;

// Include the shared types directly in the build script
include!("src/config/toml_types.rs");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=assets/default_catalog.toml");

    let out_dir = env::var("OUT_DIR")?;
    let dest_path = Path::new(&out_dir);

    // Load unified catalog
    let toml_content = include_str!("assets/default_catalog.toml");
    let catalog: Catalog = toml::from_str(toml_content)?;

    generate_provider_constants(dest_path, &catalog.providers)?;
    generate_model_constants(dest_path, &catalog.models)?;
    Ok(())
}

fn generate_provider_constants(out_dir: &Path, providers: &[ProviderData]) -> std::io::Result<()> {
    let mut output = String::new();
    output.push_str("// Generated provider constants from default_catalog.toml\n\n");

    // Generate string constants and constructor functions
    for provider in providers {
        let const_name = provider.id.to_uppercase();
        let fn_name = provider.id.to_lowercase();
        output.push_str(&format!(
            "pub const {}_ID: &str = \"{}\";\n",
            const_name, provider.id
        ));
        output.push_str(&format!(
            "#[inline]\npub fn {}() -> ProviderId {{ ProviderId(\"{}\".to_string()) }}\n\n",
            fn_name, provider.id
        ));
    }

    let dest_file = out_dir.join("generated_provider_ids.rs");
    fs::write(&dest_file, output)?;
    Ok(())
}

fn generate_model_constants(out_dir: &Path, models: &[ModelData]) -> std::io::Result<()> {
    let mut output = String::new();
    output.push_str("// Generated model constants from default_catalog.toml\n");
    output.push_str("use crate::config::model::ModelId;\n\n");

    // First pass: generate primary constants for each model
    for model in models {
        let const_name = model.id.to_lowercase().replace(['-', '.'], "_");
        let provider_fn = model.provider.to_lowercase();
        output.push_str(&format!(
            "#[inline]\npub fn {}() -> ModelId {{ ModelId {{ provider: crate::config::provider::{}(), id: \"{}\".to_string() }} }}\n",
            const_name, provider_fn, model.id
        ));
    }

    output.push_str("\n// Aliases\n");

    // Second pass: generate alias constants
    for model in models {
        let target_fn = model.id.to_lowercase().replace(['-', '.'], "_");
        for alias in &model.aliases {
            let alias_const = alias.to_lowercase().replace(['-', '.'], "_");
            output.push_str(&format!(
                "#[inline]\npub fn {alias_const}() -> ModelId {{ {target_fn}() }}\n"
            ));
        }
    }

    // Generate DEFAULT_MODEL constant - hardcoded to codex
    output
        .push_str("\n// Default model\n#[inline]\npub fn default_model() -> ModelId { codex() }\n");

    let dest_file = out_dir.join("generated_model_ids.rs");
    fs::write(&dest_file, output)?;
    Ok(())
}
