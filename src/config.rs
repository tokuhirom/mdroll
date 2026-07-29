//! Configuration.
//!
//! Precedence is **command line → environment → config file → built-in
//! default**. [`Settings`] is the resolved result; nothing downstream looks at
//! the environment again.

use crate::theme::DEFAULT_THEME;
use crate::width::WidthCalc;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The config file. Every field is optional so an absent file and an empty file
/// mean the same thing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub theme: Option<String>,
    pub default_mode: Option<String>,
    pub default_wrap: Option<bool>,
    pub width: Option<usize>,
    pub margin: Option<usize>,
    pub status: Option<bool>,
    pub double_height_headings: Option<bool>,
    pub images: Option<bool>,
    pub remote_images: Option<bool>,
    pub graphics: Option<String>,
    pub mouse: Option<bool>,
    pub east_asian_ambiguous_wide: Option<bool>,
    pub watch: Option<bool>,
    pub mermaid: Option<String>,
    #[serde(default)]
    pub keys: BTreeMap<String, Vec<String>>,
}

/// Whether the terminal is taken to speak the Kitty graphics protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphicsMode {
    /// Work it out from the environment.
    #[default]
    Auto,
    /// Assume it does, whatever the environment says. What `ssh` needs: the
    /// variables a terminal sets exist on the machine the terminal runs on,
    /// not on the one at the other end of the connection — but the escape
    /// sequences travel down the wire like any other output.
    Kitty,
    /// Never draw pictures.
    None,
}

impl GraphicsMode {
    pub fn parse(s: &str) -> Option<GraphicsMode> {
        match s.to_ascii_lowercase().as_str() {
            "auto" | "detect" => Some(GraphicsMode::Auto),
            "kitty" | "on" | "yes" | "force" => Some(GraphicsMode::Kitty),
            "none" | "off" | "no" => Some(GraphicsMode::None),
            _ => None,
        }
    }
}

/// How mermaid blocks are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MermaidMode {
    /// Box drawings where they work, `mmdc` for everything else. The default:
    /// box drawings are instant and are selectable text, so they win when they
    /// can render the diagram at all.
    #[default]
    Auto,
    /// Never launch `mmdc`.
    Text,
    /// Always render through `mmdc`, where it and terminal graphics exist.
    Image,
}

impl MermaidMode {
    pub fn parse(s: &str) -> Option<MermaidMode> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(MermaidMode::Auto),
            "text" | "boxes" => Some(MermaidMode::Text),
            "image" | "images" => Some(MermaidMode::Image),
            _ => None,
        }
    }
}

impl ConfigFile {
    pub fn load(path: &Path) -> Result<ConfigFile> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
    }
}

/// Default config path, `$XDG_CONFIG_HOME/mdroll/config.toml`.
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mdroll").join("config.toml"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub theme: String,
    pub source: bool,
    pub wrap: bool,
    pub width: usize,
    pub margin: usize,
    pub status: bool,
    pub double_height: bool,
    pub images: bool,
    /// Fetch images that live behind an `http(s)` URL. Opening a document then
    /// means talking to whichever hosts it points at, so it can be turned off.
    pub remote_images: bool,
    /// Whether to believe the terminal can draw pictures.
    pub graphics: GraphicsMode,
    pub mouse: bool,
    pub ambiguous_wide: bool,
    pub no_color: bool,
    pub watch: bool,
    pub mermaid: MermaidMode,
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: DEFAULT_THEME.into(),
            source: false,
            wrap: true,
            width: 0,
            margin: 2,
            status: false,
            double_height: true,
            images: true,
            remote_images: true,
            graphics: GraphicsMode::default(),
            mouse: false,
            ambiguous_wide: false,
            no_color: false,
            watch: false,
            mermaid: MermaidMode::default(),
            keys: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub fn calc(&self) -> WidthCalc {
        WidthCalc::new(self.ambiguous_wide)
    }

    /// Apply a config file over the built-in defaults.
    pub fn apply_file(&mut self, file: &ConfigFile) {
        if let Some(v) = &file.theme {
            self.theme = v.clone();
        }
        if let Some(v) = &file.default_mode {
            self.source = v.eq_ignore_ascii_case("source");
        }
        if let Some(v) = file.default_wrap {
            self.wrap = v;
        }
        if let Some(v) = file.width {
            self.width = v;
        }
        if let Some(v) = file.margin {
            self.margin = v;
        }
        if let Some(v) = file.status {
            self.status = v;
        }
        if let Some(v) = file.double_height_headings {
            self.double_height = v;
        }
        if let Some(v) = file.images {
            self.images = v;
        }
        if let Some(v) = file.remote_images {
            self.remote_images = v;
        }
        if let Some(v) = file.graphics.as_deref().and_then(GraphicsMode::parse) {
            self.graphics = v;
        }
        if let Some(v) = file.mouse {
            self.mouse = v;
        }
        if let Some(v) = file.east_asian_ambiguous_wide {
            self.ambiguous_wide = v;
        }
        if let Some(v) = file.watch {
            self.watch = v;
        }
        if let Some(v) = file.mermaid.as_deref().and_then(MermaidMode::parse) {
            self.mermaid = v;
        }
        if !file.keys.is_empty() {
            self.keys = file.keys.clone();
        }
    }

    /// Apply environment variables over the config file.
    pub fn apply_env(&mut self, env: &dyn Fn(&str) -> Option<String>) {
        if let Some(theme) = env("MDROLL_THEME") {
            self.theme = theme;
        }
        // NO_COLOR is honoured whatever its value, per the convention.
        if env("NO_COLOR").is_some() {
            self.no_color = true;
        }
    }
}

/// Look up an environment variable, treating empty as unset.
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_config_leaves_the_defaults_alone() {
        let file: ConfigFile = toml::from_str("").unwrap();
        let mut settings = Settings::default();
        settings.apply_file(&file);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn the_config_file_overrides_defaults() {
        let file: ConfigFile = toml::from_str(
            r#"
            theme = "dracula"
            default_mode = "source"
            default_wrap = false
            width = 100
            status = true
            east_asian_ambiguous_wide = true
            "#,
        )
        .unwrap();
        let mut settings = Settings::default();
        settings.apply_file(&file);
        assert_eq!(settings.theme, "dracula");
        assert!(settings.source);
        assert!(!settings.wrap);
        assert_eq!(settings.width, 100);
        assert!(settings.status);
        assert!(settings.calc().ambiguous_wide);
    }

    #[test]
    fn the_environment_overrides_the_config_file() {
        let file: ConfigFile = toml::from_str(r#"theme = "nord""#).unwrap();
        let mut settings = Settings::default();
        settings.apply_file(&file);
        settings.apply_env(&|name| match name {
            "MDROLL_THEME" => Some("gruvbox".into()),
            _ => None,
        });
        assert_eq!(settings.theme, "gruvbox");
    }

    #[test]
    fn no_color_is_honoured_even_when_empty_is_not() {
        let mut settings = Settings::default();
        settings.apply_env(&|name| (name == "NO_COLOR").then(|| "1".to_string()));
        assert!(settings.no_color);
    }

    #[test]
    fn unknown_config_keys_are_an_error_rather_than_a_silent_typo() {
        assert!(toml::from_str::<ConfigFile>("thmee = \"dracula\"").is_err());
    }

    #[test]
    fn the_mermaid_mode_is_read_from_the_config() {
        let file: ConfigFile = toml::from_str(r#"mermaid = "image""#).unwrap();
        let mut settings = Settings::default();
        settings.apply_file(&file);
        assert_eq!(settings.mermaid, MermaidMode::Image);
    }

    #[test]
    fn an_unrecognised_mermaid_mode_leaves_the_default_alone() {
        let file: ConfigFile = toml::from_str(r#"mermaid = "sideways""#).unwrap();
        let mut settings = Settings::default();
        settings.apply_file(&file);
        assert_eq!(settings.mermaid, MermaidMode::Auto);
    }

    #[test]
    fn key_overrides_are_read_as_lists() {
        let file: ConfigFile = toml::from_str(
            r#"
            [keys]
            quit = ["q", "Esc"]
            toggle_wrap = ["w"]
            "#,
        )
        .unwrap();
        assert_eq!(file.keys["quit"], vec!["q", "Esc"]);
    }
}
