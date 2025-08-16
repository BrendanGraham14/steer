use std::path::PathBuf;

/// Standardized application directories for Steer.
///
/// - Project-level: ./.steer
/// - User-level config: uses OS-specific dirs
/// - User-level data: uses OS-specific dirs
pub struct AppPaths;

impl AppPaths {
    /// Return the project-level .steer directory (relative to current working dir)
    pub fn project_dir() -> PathBuf {
        PathBuf::from(".steer")
    }

    /// Return the project-level catalog path: ./.steer/catalog.toml
    pub fn project_catalog() -> PathBuf {
        Self::project_dir().join("catalog.toml")
    }

    /// Return the user-level config directory (platform-specific)
    pub fn user_config_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "steer").map(|d| d.config_dir().to_path_buf())
    }

    /// Return the user-level data directory (platform-specific)
    pub fn user_data_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "steer").map(|d| d.data_dir().to_path_buf())
    }

    /// Return the user-level catalog path (platform-specific)
    pub fn user_catalog() -> Option<PathBuf> {
        Self::user_config_dir().map(|d| d.join("catalog.toml"))
    }

    /// Standard discovery order for catalog files
    /// Project catalog first, then user catalog
    pub fn discover_catalogs() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        paths.push(Self::project_catalog());
        if let Some(user_cat) = Self::user_catalog() {
            paths.push(user_cat);
        }
        paths
    }

    /// Standard discovery order for session config files.
    pub fn discover_session_configs() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        // ./.steer/session.toml, then user-level session.toml
        let project = Self::project_dir().join("session.toml");
        if project.exists() {
            paths.push(project);
        }
        if let Some(user_cfg) = Self::user_config_dir() {
            let user = user_cfg.join("session.toml");
            if user.exists() {
                paths.push(user);
            }
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_paths_are_static() {
        assert_eq!(AppPaths::project_dir(), PathBuf::from(".steer"));
        assert_eq!(
            AppPaths::project_catalog(),
            PathBuf::from(".steer/catalog.toml")
        );
    }

    #[test]
    fn discover_catalogs_order_is_deterministic() {
        let catalogs = AppPaths::discover_catalogs();
        // Expect exactly 1 or 2 entries: project first, optional user second
        assert!(catalogs.len() == 1 || catalogs.len() == 2);
        assert_eq!(catalogs[0], PathBuf::from(".steer/catalog.toml"));
        if catalogs.len() == 2 {
            let expected_user =
                AppPaths::user_catalog().expect("user catalog path should exist when returned");
            assert_eq!(catalogs[1], expected_user);
        }
    }

    #[test]
    fn discover_session_configs_order_is_deterministic() {
        let configs = AppPaths::discover_session_configs();
        // Expect 0, 1, or 2 entries and preserve order
        assert!(configs.len() <= 2);
        if configs.len() == 2 {
            // When both exist, project comes before user
            assert_eq!(configs[0], PathBuf::from(".steer/session.toml"));
            let expected_user = AppPaths::user_config_dir().unwrap().join("session.toml");
            assert_eq!(configs[1], expected_user);
        } else if configs.len() == 1 {
            // Single entry may be either project or user-level file
            let single = &configs[0];
            let is_project = *single == PathBuf::from(".steer/session.toml");
            let is_user = AppPaths::user_config_dir()
                .map(|d| single == &d.join("session.toml"))
                .unwrap_or(false);
            assert!(is_project || is_user);
        }
    }
}
