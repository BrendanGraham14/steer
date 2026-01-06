use std::collections::HashMap;

use crate::error::{Error, Result};

pub fn create_session_store_path() -> Result<std::path::PathBuf> {
    let home_dir = dirs::home_dir()
        .ok_or_else(|| Error::Configuration("Could not determine home directory".to_string()))?;
    let db_path = home_dir.join(".steer").join("sessions.db");
    Ok(db_path)
}

pub fn parse_metadata(metadata_str: Option<&str>) -> Result<HashMap<String, String>> {
    let mut metadata = HashMap::new();

    if let Some(meta_str) = metadata_str {
        for pair in meta_str.split(',') {
            let parts: Vec<&str> = pair.split('=').collect();
            if parts.len() != 2 {
                return Err(Error::Configuration(
                    "Invalid metadata format. Expected key=value pairs separated by commas"
                        .to_string(),
                ));
            }
            metadata.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
        }
    }

    Ok(metadata)
}
