//! User configuration: a hand-editable JSON file with explicit validation.
//!
//! Configuration is user input, so every field is validated with a readable error that
//! names the offending field. Unknown fields are ignored so an older build can still
//! read a file written by a newer one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub font_family: String,
    pub font_size: f32,
    pub scrollback_lines: usize,
    pub keepalive_seconds: u64,
    pub tmux: TmuxSettings,
    pub keybindings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TmuxSettings {
    pub status: bool,
    pub mouse: bool,
    pub history_limit: u32,
    pub default_terminal: String,
}

impl Default for TmuxSettings {
    fn default() -> Self {
        Self {
            status: false,
            mouse: false,
            history_limit: 100_000,
            default_terminal: "tmux-256color".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            font_family: "Cascadia Mono".into(),
            font_size: 14.0,
            scrollback_lines: 100_000,
            keepalive_seconds: 30,
            tmux: TmuxSettings::default(),
            keybindings: default_keybindings(),
        }
    }
}

fn default_keybindings() -> BTreeMap<String, String> {
    [
        ("new_tab", "Ctrl+Shift+T"),
        ("close_pane", "Ctrl+Shift+W"),
        ("next_tab", "Ctrl+Tab"),
        ("previous_tab", "Ctrl+Shift+Tab"),
        ("reconnect_all", "Ctrl+Shift+R"),
        ("toggle_sidebar", "Ctrl+Shift+B"),
        ("search", "Ctrl+F"),
    ]
    .into_iter()
    .map(|(action, keys)| (action.to_string(), keys.to_string()))
    .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config field `{field}`: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl Config {
    /// Reject values that would produce a broken UI or an unbounded buffer.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::Invalid {
                field: "version",
                reason: format!("expected {CONFIG_VERSION}, found {}", self.version),
            });
        }
        if !(6.0..=72.0).contains(&self.font_size) {
            return Err(ConfigError::Invalid {
                field: "font_size",
                reason: format!("must be between 6 and 72, found {}", self.font_size),
            });
        }
        if !(1_000..=1_000_000).contains(&self.scrollback_lines) {
            return Err(ConfigError::Invalid {
                field: "scrollback_lines",
                reason: format!(
                    "must be between 1000 and 1000000, found {}",
                    self.scrollback_lines
                ),
            });
        }
        if !(5..=600).contains(&self.keepalive_seconds) {
            return Err(ConfigError::Invalid {
                field: "keepalive_seconds",
                reason: format!(
                    "must be between 5 and 600, found {}",
                    self.keepalive_seconds
                ),
            });
        }
        if !(1_000..=1_000_000).contains(&self.tmux.history_limit) {
            return Err(ConfigError::Invalid {
                field: "tmux.history_limit",
                reason: format!(
                    "must be between 1000 and 1000000, found {}",
                    self.tmux.history_limit
                ),
            });
        }
        if self.font_family.trim().is_empty() {
            return Err(ConfigError::Invalid {
                field: "font_family",
                reason: "must not be empty".into(),
            });
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = serde_json::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(self).expect("config is serializable");
        std::fs::write(path, text).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// `<config dir>/sentinel/config.json`, honouring `SENTINEL_CONFIG_DIR` for tests.
pub fn config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("SENTINEL_CONFIG_DIR") {
        return PathBuf::from(dir).join("config.json");
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA").unwrap_or_default();
        PathBuf::from(base).join("sentinel").join("config.json")
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        base.join("sentinel").join("config.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("missing.json")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn edits_roundtrip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = Config::default();
        config.font_family = "JetBrains Mono".into();
        config.font_size = 16.0;
        config.keybindings.insert("new_tab".into(), "Ctrl+T".into());
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.font_family, "JetBrains Mono");
        assert_eq!(loaded.font_size, 16.0);
        assert_eq!(
            loaded.keybindings.get("new_tab").map(String::as_str),
            Some("Ctrl+T")
        );
    }

    #[test]
    fn invalid_values_name_the_field() {
        let mut config = Config::default();
        config.font_size = 200.0;
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("font_size"), "{error}");

        let mut config = Config::default();
        config.scrollback_lines = 10;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("scrollback_lines"));

        let mut config = Config::default();
        config.keepalive_seconds = 1;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("keepalive_seconds"));

        let mut config = Config::default();
        config.tmux.history_limit = 10;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("tmux.history_limit"));

        let mut config = Config::default();
        config.font_family = "   ".into();
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("font_family"));
    }

    #[test]
    fn malformed_json_reports_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        assert!(error.contains("config.json"), "{error}");
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compatibility() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            br#"{"version":1,"font_size":15.0,"from_the_future":true}"#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.font_size, 15.0);
    }

    #[test]
    fn every_documented_field_has_a_default() {
        let json = serde_json::to_value(Config::default()).unwrap();
        for field in [
            "version",
            "font_family",
            "font_size",
            "scrollback_lines",
            "keepalive_seconds",
            "tmux",
            "keybindings",
        ] {
            assert!(json.get(field).is_some(), "missing default for {field}");
        }
    }
}
