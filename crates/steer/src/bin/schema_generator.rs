use schemars::schema_for;
use std::fs;
use std::path::Path;
use steer::session_config::PartialSessionConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the JSON schema for PartialSessionConfig
    let mut schema = schema_for!(PartialSessionConfig);

    // Add schema metadata
    schema.ensure_object().insert(
        "title".to_string(),
        serde_json::json!("Steer Session Configuration"),
    );
    schema.ensure_object().insert("description".to_string(), serde_json::json!("Configuration file for Steer sessions, including workspace settings, tool configurations, and AI behavior customization."));

    // Convert to pretty-printed JSON
    let json = serde_json::to_string_pretty(&schema)?;

    // Ensure schemas directory exists
    let schema_dir = Path::new("schemas");
    fs::create_dir_all(schema_dir)?;

    // Write to file
    let schema_path = schema_dir.join("session.schema.json");
    fs::write(&schema_path, json)?;

    println!("Schema written to {}", schema_path.display());

    Ok(())
}
