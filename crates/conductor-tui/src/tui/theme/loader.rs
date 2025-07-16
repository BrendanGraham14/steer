//! Theme loading functionality

use super::{RawTheme, Theme, ThemeError};
use directories::ProjectDirs;
use std::fs;
use std::path::{Path, PathBuf};

/// Bundled themes included with the application
const BUNDLED_THEMES: &[(&str, &str)] = &[
    // Dark themes
    (
        "catppuccin-mocha",
        include_str!("../../../themes/catppuccin-mocha.toml"),
    ),
    ("dracula", include_str!("../../../themes/dracula.toml")),
    (
        "gruvbox-dark",
        include_str!("../../../themes/gruvbox-dark.toml"),
    ),
    ("nord", include_str!("../../../themes/nord.toml")),
    ("one-dark", include_str!("../../../themes/one-dark.toml")),
    (
        "solarized-dark",
        include_str!("../../../themes/solarized-dark.toml"),
    ),
    (
        "tokyo-night-storm",
        include_str!("../../../themes/tokyo-night-storm.toml"),
    ),
    // Light themes
    (
        "catppuccin-latte",
        include_str!("../../../themes/catppuccin-latte.toml"),
    ),
    (
        "github-light",
        include_str!("../../../themes/github-light.toml"),
    ),
    (
        "gruvbox-light",
        include_str!("../../../themes/gruvbox-light.toml"),
    ),
    ("one-light", include_str!("../../../themes/one-light.toml")),
    (
        "solarized-light",
        include_str!("../../../themes/solarized-light.toml"),
    ),
];

/// Theme loader responsible for finding and loading theme files
pub struct ThemeLoader {
    search_paths: Vec<PathBuf>,
}

impl ThemeLoader {
    /// Create a new theme loader with default search paths
    pub fn new() -> Self {
        let mut search_paths = Vec::new();

        // Add paths from directories crate
        if let Some(proj_dirs) = ProjectDirs::from("", "", "conductor") {
            // Config directory (e.g., ~/.config/conductor/themes on Linux)
            search_paths.push(proj_dirs.config_dir().join("themes"));

            // Data directory as fallback (e.g., ~/.local/share/conductor/themes on Linux)
            search_paths.push(proj_dirs.data_dir().join("themes"));
        }

        Self { search_paths }
    }

    /// Add a custom search path
    pub fn add_search_path(&mut self, path: PathBuf) {
        self.search_paths.push(path);
    }

    /// Load a theme by name
    pub fn load_theme(&self, name: &str) -> Result<Theme, ThemeError> {
        // First check if it's a bundled theme
        for (theme_name, theme_content) in BUNDLED_THEMES {
            if theme_name == &name {
                let raw_theme: RawTheme = toml::from_str(theme_content)?;
                return raw_theme.into_theme();
            }
        }

        // Try to find the theme file in the filesystem
        let theme_file = self.find_theme_file(name)?;

        // Read and parse the theme file
        let content = fs::read_to_string(&theme_file)?;
        let raw_theme: RawTheme = toml::from_str(&content)?;

        // Validate theme name matches
        if raw_theme.name.to_lowercase() != name.to_lowercase() {
            return Err(ThemeError::Validation(format!(
                "Theme name mismatch: expected '{}', found '{}'",
                name, raw_theme.name
            )));
        }

        // Convert to usable theme
        raw_theme.into_theme()
    }

    /// Load a theme from a specific file path
    pub fn load_theme_from_path(&self, path: &Path) -> Result<Theme, ThemeError> {
        let content = fs::read_to_string(path)?;
        let raw_theme: RawTheme = toml::from_str(&content)?;
        raw_theme.into_theme()
    }

    /// List all available themes
    pub fn list_themes(&self) -> Vec<String> {
        let mut themes = Vec::new();

        // Add bundled themes
        for (theme_name, _) in BUNDLED_THEMES {
            themes.push(theme_name.to_string());
        }

        // Add filesystem themes
        for search_path in &self.search_paths {
            if let Ok(entries) = fs::read_dir(search_path) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_file() {
                            if let Some(name) = entry.file_name().to_str() {
                                if name.ends_with(".toml") {
                                    let theme_name = name.trim_end_matches(".toml");
                                    if !themes.contains(&theme_name.to_string()) {
                                        themes.push(theme_name.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        themes.sort();
        themes
    }

    /// Find a theme file by name in the search paths
    fn find_theme_file(&self, name: &str) -> Result<PathBuf, ThemeError> {
        let filename = format!("{name}.toml");

        for search_path in &self.search_paths {
            let theme_path = search_path.join(&filename);
            if theme_path.exists() {
                return Ok(theme_path);
            }
        }

        Err(ThemeError::Validation(format!(
            "Theme '{name}' not found in bundled themes or filesystem"
        )))
    }
}

impl Default for ThemeLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_theme() {
        let temp_dir = TempDir::new().unwrap();
        let theme_path = temp_dir.path().join("test-theme.toml");

        let theme_content = r##"
name = "test-theme"

[palette]
background = "#282828"
foreground = "#ebdbb2"

[components]
status_bar = { fg = "foreground", bg = "background" }

[colors]
bg = "#282828"
fg = "#ebdbb2"

[styles]
border = { fg = "fg" }
text = { fg = "fg" }
"##;
        let mut file = std::fs::File::create(&theme_path).unwrap();
        std::io::Write::write_all(&mut file, theme_content.as_bytes()).unwrap();

        let mut loader = ThemeLoader::new();
        loader.add_search_path(temp_dir.path().to_path_buf());

        let theme = loader.load_theme("test-theme").unwrap();
        assert_eq!(theme.name, "test-theme");
    }

    #[test]
    fn test_load_bundled_themes() {
        let loader = ThemeLoader::new();

        // Iterate through all theme files in the themes directory
        let themes_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let entries = std::fs::read_dir(&themes_dir)
            .unwrap_or_else(|e| panic!("Failed to read themes directory: {e}"));

        let mut errors = Vec::new();

        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();

            // Skip non-TOML files
            if path.extension().is_none_or(|ext| ext != "toml") {
                continue;
            }

            let theme_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("Invalid theme filename");

            match loader.load_theme(theme_id) {
                Ok(theme) => {
                    // Verify the theme loaded successfully
                    if theme.name.is_empty() {
                        errors.push(format!("Theme '{theme_id}' has empty name"));
                    }
                }
                Err(e) => {
                    // Provide detailed error information
                    let error_msg = match &e {
                        ThemeError::ColorNotFound(color) => {
                            format!(
                                "Theme '{theme_id}' references undefined color '{color}'. Check that this color is defined in either the 'palette' or 'colors' section."
                            )
                        }
                        ThemeError::InvalidColor(msg) => {
                            format!("Theme '{theme_id}' has invalid color: {msg}")
                        }
                        ThemeError::Parse(parse_err) => {
                            format!("Theme '{theme_id}' has invalid TOML syntax: {parse_err}")
                        }
                        ThemeError::Validation(msg) => {
                            format!("Theme '{theme_id}' validation error: {msg}")
                        }
                        ThemeError::Io(io_err) => {
                            format!("Theme '{theme_id}' I/O error: {io_err}")
                        }
                    };
                    errors.push(error_msg);
                }
            }
        }

        if !errors.is_empty() {
            panic!("Theme loading errors:\n\n{}", errors.join("\n\n"));
        }
    }

    #[test]
    fn test_list_themes() {
        let temp_dir = TempDir::new().unwrap();
        let theme1_path = temp_dir.path().join("theme1.toml");
        let theme2_path = temp_dir.path().join("theme2.toml");

        let theme_content = r#"name = "Test"
[colors]
[styles]
"#;
        std::fs::write(&theme1_path, theme_content).unwrap();
        std::fs::write(&theme2_path, theme_content).unwrap();

        let mut loader = ThemeLoader::new();
        loader.add_search_path(temp_dir.path().to_path_buf());

        let themes = loader.list_themes();
        assert!(themes.contains(&"theme1".to_string()));
        assert!(themes.contains(&"theme2".to_string()));
        // Also check that bundled themes are included
        assert!(themes.contains(&"catppuccin-mocha".to_string()));
        assert!(themes.contains(&"catppuccin-latte".to_string()));
    }

    #[test]
    fn test_theme_not_found() {
        let loader = ThemeLoader::new();
        let result = loader.load_theme("non-existent-theme");
        assert!(matches!(result, Err(ThemeError::Validation(_))));
    }

    #[test]
    fn test_bundled_themes_validation() {
        use super::super::{ColorValue, Component, RawTheme};

        // Test that all bundled themes are valid and follow best practices
        for (theme_name, theme_content) in BUNDLED_THEMES {
            let raw_theme: RawTheme = toml::from_str(theme_content)
                .unwrap_or_else(|e| panic!("Failed to parse theme '{theme_name}': {e}"));

            // Ensure theme has the required palette colors
            assert!(
                raw_theme.palette.contains_key("background"),
                "Theme '{theme_name}' missing 'background' in palette"
            );
            assert!(
                raw_theme.palette.contains_key("foreground"),
                "Theme '{theme_name}' missing 'foreground' in palette"
            );

            // Verify components don't use direct hex colors (should use palette references)
            for (component_name, style) in &raw_theme.components {
                if let Some(fg) = &style.fg {
                    match fg {
                        ColorValue::Direct(color) if color.starts_with('#') => {
                            panic!(
                                "Theme '{theme_name}' component '{component_name:?}' uses direct hex color '{color}' instead of palette reference"
                            );
                        }
                        _ => {} // Palette reference or named color is fine
                    }
                }
                if let Some(bg) = &style.bg {
                    match bg {
                        ColorValue::Direct(color) if color.starts_with('#') => {
                            panic!(
                                "Theme '{theme_name}' component '{component_name:?}' uses direct hex color '{color}' instead of palette reference"
                            );
                        }
                        _ => {} // Palette reference or named color is fine
                    }
                }
            }

            // Verify theme can be loaded successfully (includes resolution validation)
            let theme = raw_theme
                .into_theme()
                .unwrap_or_else(|e| panic!("Failed to convert theme '{theme_name}': {e}"));

            // Verify critical components are defined
            let critical_components = [
                Component::StatusBar,
                Component::ErrorText,
                Component::AssistantMessage,
                Component::UserMessage,
                Component::InputPanelBorder,
                Component::ChatListBorder,
                Component::SelectionHighlight,
            ];

            for component in critical_components {
                assert!(
                    theme.styles.contains_key(&component),
                    "Theme '{theme_name}' missing critical component: {component:?}"
                );
            }
        }
    }

    #[test]
    fn test_theme_palette_references_resolve() {
        let loader = ThemeLoader::new();

        // Test a few themes to ensure palette references resolve to actual colors
        for theme_name in ["catppuccin-mocha", "gruvbox-dark", "solarized-light"] {
            let theme = loader
                .load_theme(theme_name)
                .unwrap_or_else(|e| panic!("Failed to load theme '{theme_name}': {e}"));

            // Check that critical components have resolved colors (not None)
            let status_bar = theme
                .styles
                .get(&super::super::Component::StatusBar)
                .expect("StatusBar component missing");

            // StatusBar should at least have a foreground color
            if status_bar.fg.is_none() && status_bar.bg.is_none() {
                panic!("Theme '{theme_name}' StatusBar has no colors defined");
            }
        }
    }
}
