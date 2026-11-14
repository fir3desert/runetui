//! Persistent settings.
//!
//! runetui only writes its own configuration, stored under the platform config
//! directory (`~/.config/runetui/config.toml` on Linux). Displayed files are
//! read into memory; they are only modified through the external editor that the
//! user explicitly launches with `e`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::filesystem::explorer::{DEFAULT_WIDTH_PCT, MAX_WIDTH_PCT, MIN_WIDTH_PCT};
use crate::keymap::{self, KeySpec};
use crate::ui::icons::IconStyle;
use crate::ui::layout::{DEFAULT_TERMINAL_PCT, MAX_TERMINAL_PCT, MIN_TERMINAL_PCT};
use crate::ui::themes::ThemeName;

pub const CONFIG_DIR: &str = "runetui";
pub const CONFIG_FILE: &str = "config.toml";
pub const DEFAULT_THEME: &str = "catppuccin-mocha";
pub const DEFAULT_ICONS: &str = "unicode";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Project directory opened last time, used for the bare `rtui` command.
    pub last_directory: Option<PathBuf>,
    /// Theme slug, e.g. `catppuccin-mocha`.
    pub theme: String,
    /// Explorer icon style: `nerd`, `unicode` or `ascii`.
    pub icons: String,
    pub show_hidden: bool,
    pub respect_gitignore: bool,
    pub explorer_width: u16,
    pub show_explorer: bool,
    /// Edit files inside the document pane (a real editor in a pseudo-terminal)
    /// instead of suspending the TUI.
    pub embedded_editor: bool,
    /// Show the terminal panel at the bottom.
    pub show_terminal: bool,
    /// Height of the terminal panel as a percentage of the body.
    pub terminal_height: u16,
    /// Capture the mouse: pane-limited text selection, click to focus and
    /// wheel scrolling.  Turn it off to get the terminal's own selection back.
    pub mouse: bool,
    /// Prefix key for two-key shortcuts (tmux style), e.g. `ctrl+b`.
    pub prefix: String,
    /// Shortcut bindings, one entry per action (see `keymap`).  Keep this the
    /// last field so TOML writes it as a table at the end of the file.
    pub keys: BTreeMap<String, KeySpec>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            last_directory: None,
            theme: DEFAULT_THEME.to_string(),
            icons: DEFAULT_ICONS.to_string(),
            show_hidden: false,
            respect_gitignore: true,
            explorer_width: DEFAULT_WIDTH_PCT,
            show_explorer: true,
            embedded_editor: true,
            show_terminal: true,
            terminal_height: DEFAULT_TERMINAL_PCT,
            mouse: true,
            prefix: keymap::DEFAULT_PREFIX.to_string(),
            keys: keymap::default_specs(),
        }
    }
}

impl Config {
    pub fn theme_name(&self) -> ThemeName {
        ThemeName::from_slug(&self.theme).unwrap_or(ThemeName::CatppuccinMocha)
    }

    pub fn set_theme(&mut self, name: ThemeName) {
        self.theme = name.slug().to_string();
    }

    pub fn icon_style(&self) -> IconStyle {
        IconStyle::from_slug(&self.icons).unwrap_or(IconStyle::Nerd)
    }

    pub fn set_icon_style(&mut self, style: IconStyle) {
        self.icons = style.slug().to_string();
    }

    /// Configuration path, following the platform convention.
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|directory| directory.join(CONFIG_DIR).join(CONFIG_FILE))
    }

    /// Load settings, falling back to defaults for missing or malformed files.
    /// A broken config must never prevent reading documents.
    pub fn load() -> Self {
        match Self::path() {
            Some(path) => Self::load_from(&path).unwrap_or_default(),
            None => Self::default(),
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let mut config: Config =
            toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))?;
        config.normalize();
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        match Self::path() {
            Some(path) => self.save_to(&path),
            None => Ok(()),
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let mut config = self.clone();
        config.normalize();
        let text = toml::to_string_pretty(&config).context("cannot serialise settings")?;
        fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
    }

    fn normalize(&mut self) {
        if ThemeName::from_slug(&self.theme).is_none() {
            self.theme = DEFAULT_THEME.to_string();
        }
        if IconStyle::from_slug(&self.icons).is_none() {
            self.icons = DEFAULT_ICONS.to_string();
        }
        if self
            .last_directory
            .as_ref()
            .is_some_and(|dir| !dir.is_dir())
        {
            self.last_directory = None;
        }
        self.explorer_width = self.explorer_width.clamp(MIN_WIDTH_PCT, MAX_WIDTH_PCT);
        self.terminal_height = self
            .terminal_height
            .clamp(MIN_TERMINAL_PCT, MAX_TERMINAL_PCT);
        // Actions missing from an older or hand-written file get their
        // defaults so the saved file always lists every shortcut.
        for (name, spec) in keymap::default_specs() {
            self.keys.entry(name).or_insert(spec);
        }
    }
}

/// Resolve the initial project directory.
///
/// Priority: explicit CLI path, then the last opened project, then the current
/// working directory.
pub fn resolve_root(cli_path: Option<&Path>, config: &Config) -> PathBuf {
    if let Some(path) = cli_path {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        };
        if absolute.is_dir() {
            return absolute;
        }
        if absolute.is_file() {
            if let Some(parent) = absolute.parent() {
                return parent.to_path_buf();
            }
        }
        // A non-existent path with a file-looking name: open its parent so the
        // interface still starts, even though the file itself is missing.
        if absolute.extension().is_some() {
            if let Some(parent) = absolute.parent().filter(|parent| parent.is_dir()) {
                return parent.to_path_buf();
            }
        }
    }
    if let Some(directory) = &config.last_directory {
        if directory.is_dir() {
            return directory.clone();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// If the CLI argument was a file, return its absolute path.
pub fn resolve_initial_file(cli_path: Option<&Path>) -> Option<PathBuf> {
    let path = cli_path?;
    if !path.is_file() {
        return None;
    }
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        std::env::current_dir().ok().map(|cwd| cwd.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.theme_name(), ThemeName::CatppuccinMocha);
        assert!(config.respect_gitignore);
        assert!(config.show_explorer);
        assert_eq!(config.explorer_width, DEFAULT_WIDTH_PCT);
    }

    #[test]
    fn roundtrips_through_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(CONFIG_FILE);
        let mut config = Config::default();
        config.set_theme(ThemeName::Gruvbox);
        config.show_hidden = true;
        config.explorer_width = 40;
        config.last_directory = Some(dir.path().to_path_buf());
        config.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.theme_name(), ThemeName::Gruvbox);
        assert!(loaded.show_hidden);
        assert_eq!(loaded.explorer_width, 40);
        assert_eq!(loaded.last_directory.as_deref(), Some(dir.path()));
    }

    #[test]
    fn invalid_values_are_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(
            &path,
            "theme = \"not-a-theme\"\nexplorer_width = 2\nlast_directory = \"/nope/missing\"\n",
        )
        .unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.theme, DEFAULT_THEME);
        assert_eq!(loaded.explorer_width, MIN_WIDTH_PCT);
        assert!(loaded.last_directory.is_none());
    }

    #[test]
    fn terminal_and_key_settings_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        let mut config = Config::default();
        config.show_terminal = false;
        config.terminal_height = 45;
        config.mouse = false;
        config.prefix = "ctrl+a".to_string();
        config
            .keys
            .insert("quit".to_string(), KeySpec::Many(vec!["x".into(), "ctrl+q".into()]));
        config.save_to(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[keys]"), "shortcuts are written as a table");
        assert!(text.contains("explorer_shrink"), "every action is listed");

        let loaded = Config::load_from(&path).unwrap();
        assert!(!loaded.show_terminal);
        assert_eq!(loaded.terminal_height, 45);
        assert!(!loaded.mouse);
        assert_eq!(loaded.prefix, "ctrl+a");
        assert_eq!(
            loaded.keys.get("quit"),
            Some(&KeySpec::Many(vec!["x".into(), "ctrl+q".into()]))
        );
    }

    #[test]
    fn partial_key_tables_are_completed_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(&path, "terminal_height = 3\n[keys]\nquit = \"x\"\n").unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.terminal_height, MIN_TERMINAL_PCT);
        assert_eq!(loaded.keys.get("quit"), Some(&KeySpec::One("x".into())));
        assert!(loaded.keys.contains_key("toggle_terminal"));
        assert!(loaded.show_terminal, "new settings default on for old files");
    }

    #[test]
    fn broken_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE);
        fs::write(&path, "this is not toml {{{").unwrap();
        let loaded = Config::load_from(&path).unwrap_or_default();
        assert_eq!(loaded.theme, DEFAULT_THEME);
    }

    #[test]
    fn resolves_relative_file_roots() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        fs::write(&file, b"# Note\n").unwrap();
        let config = Config::default();
        assert_eq!(resolve_root(Some(&file), &config), dir.path());
        assert_eq!(
            resolve_initial_file(Some(&file)).as_deref(),
            Some(file.as_path())
        );
        assert_eq!(
            resolve_root(Some(dir.path()), &config),
            dir.path().to_path_buf()
        );
    }
}
