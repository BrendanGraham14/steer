use conductor_cli::session_config::PartialSessionConfig;
use schemars::schema_for;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the JSON schema for PartialSessionConfig
    let mut schema = schema_for!(PartialSessionConfig);

    // Add schema metadata
    if let Some(metadata) = schema.schema.metadata.as_mut() {
        metadata.title = Some("Conductor Session Configuration".to_string());
        metadata.description = Some("Configuration file for Conductor sessions, including workspace settings, tool configurations, and AI behavior customization.".to_string());
    } else {
        schema.schema.metadata = Some(Box::new(schemars::schema::Metadata {
            title: Some("Conductor Session Configuration".to_string()),
            description: Some("Configuration file for Conductor sessions, including workspace settings, tool configurations, and AI behavior customization.".to_string()),
            ..Default::default()
        }));
    }

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
