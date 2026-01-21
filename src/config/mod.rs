use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Get the XDG config path (~/.config/gsd/config.toml)
pub fn xdg_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("gsd").join("config.toml"))
}

/// Get the default config path (XDG or fallback)
fn default_config_path() -> PathBuf {
    xdg_config_path().unwrap_or_else(|| PathBuf::from("/etc/gsd/config.toml"))
}
const DEFAULT_SCHEMA_VERSION: &str = "1";
const DEFAULT_INTERVAL_SECONDS: u64 = 60;

fn default_schema_version() -> String {
    DEFAULT_SCHEMA_VERSION.to_string()
}

fn default_logging_level() -> String {
    "info".to_string()
}

fn default_logging_console() -> bool {
    true
}

fn default_interval_seconds() -> u64 {
    DEFAULT_INTERVAL_SECONDS
}

fn default_ignore_patterns() -> Vec<String> {
    vec![
        "*.db-wal".to_string(),
        "*.db-shm".to_string(),
        "*.db-journal".to_string(),
    ]
}

fn default_author_name() -> String {
    "gsd".to_string()
}

fn default_author_email() -> String {
    "gsd@local".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub git: GitConfig,

    #[serde(default)]
    pub targets: Vec<TargetConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_logging_level")]
    pub level: String,

    #[serde(default)]
    pub directory: Option<PathBuf>,

    #[serde(default = "default_logging_console")]
    pub console: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_logging_level(),
            directory: None,
            console: default_logging_console(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "default_author_name")]
    pub author_name: String,

    #[serde(default = "default_author_email")]
    pub author_email: String,

    #[serde(default = "default_ignore_patterns")]
    pub default_ignore_patterns: Vec<String>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            author_name: default_author_name(),
            author_email: default_author_email(),
            default_ignore_patterns: default_ignore_patterns(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    /// Directory path to monitor (also serves as unique identifier)
    pub path: PathBuf,

    /// Commit interval in seconds (overrides global default)
    #[serde(default = "default_interval_seconds")]
    pub interval_seconds: u64,

    /// Additional gitignore patterns for this target
    #[serde(default)]
    pub ignore_patterns: Vec<String>,

    /// Whether this target is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl TargetConfig {
    /// Returns a display name for this target (directory name)
    pub fn name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_else(|| self.path.to_str().unwrap_or("unknown"))
    }
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathKind {
    Explicit,
    Env,
    Default,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config {path}: {source}")]
    ParseToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid configuration: {0}")]
    Invalid(String),
}

impl Config {
    pub fn resolve_path(cli_path: Option<&Path>) -> (PathBuf, ConfigPathKind) {
        if let Some(p) = cli_path {
            (p.to_path_buf(), ConfigPathKind::Explicit)
        } else if let Ok(env_path) = env::var("GSD_CONFIG") {
            (PathBuf::from(env_path), ConfigPathKind::Env)
        } else {
            (default_config_path(), ConfigPathKind::Default)
        }
    }

    /// Ensure config file exists, creating default if needed
    pub fn ensure_config_exists(cli_path: Option<&Path>) -> Result<PathBuf, ConfigError> {
        let (path, _kind) = Self::resolve_path(cli_path);

        if !path.exists() {
            // Create parent directories
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            // Write default config
            fs::write(&path, Self::default_config_toml()).map_err(|source| ConfigError::Io {
                path: path.clone(),
                source,
            })?;
        }

        Ok(path)
    }

    /// Load config, creating default if it doesn't exist
    pub fn load_or_create(cli_path: Option<&Path>) -> Result<(Self, PathBuf), ConfigError> {
        let path = Self::ensure_config_exists(cli_path)?;

        let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;

        let mut config: Config = toml::from_str(&raw).map_err(|source| ConfigError::ParseToml {
            path: path.clone(),
            source,
        })?;

        config.apply_env_overrides();
        // Don't validate here - allow empty targets for new configs

        Ok((config, path))
    }

    /// Save config to file
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Invalid(format!("failed to serialize config: {}", e)))?;
        fs::write(path, content).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Add a target to the config
    pub fn add_target(&mut self, target: TargetConfig) -> Result<(), ConfigError> {
        // Check for duplicates
        if self.targets.iter().any(|t| t.path == target.path) {
            return Err(ConfigError::Invalid(format!(
                "target already exists: {}",
                target.path.display()
            )));
        }
        self.targets.push(target);
        Ok(())
    }

    /// Remove a target from the config by path
    pub fn remove_target(&mut self, path: &Path) -> Result<TargetConfig, ConfigError> {
        let idx = self
            .targets
            .iter()
            .position(|t| t.path == path)
            .ok_or_else(|| ConfigError::Invalid(format!("target not found: {}", path.display())))?;
        Ok(self.targets.remove(idx))
    }

    /// Find a target by path
    pub fn find_target(&self, path: &Path) -> Option<&TargetConfig> {
        self.targets.iter().find(|t| t.path == path)
    }

    /// Find a target by path (mutable)
    pub fn find_target_mut(&mut self, path: &Path) -> Option<&mut TargetConfig> {
        self.targets.iter_mut().find(|t| t.path == path)
    }

    pub fn load_from_sources(cli_path: Option<&Path>) -> Result<Self, ConfigError> {
        let (path, _kind) = Self::resolve_path(cli_path);

        let raw = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;

        let mut config: Config = toml::from_str(&raw).map_err(|source| ConfigError::ParseToml {
            path: path.clone(),
            source,
        })?;

        config.apply_env_overrides();
        config.validate()?;

        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(level) = env::var("GSD_LOG_LEVEL") {
            if !level.trim().is_empty() {
                self.logging.level = level;
            }
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_for_daemon(false)
    }

    /// Validate config, optionally requiring at least one target (for daemon mode)
    pub fn validate_for_daemon(&self, require_targets: bool) -> Result<(), ConfigError> {
        if self.schema_version != DEFAULT_SCHEMA_VERSION {
            return Err(ConfigError::Invalid(format!(
                "unsupported schema_version {}, expected {}",
                self.schema_version, DEFAULT_SCHEMA_VERSION
            )));
        }

        if require_targets && self.targets.is_empty() {
            return Err(ConfigError::Invalid(
                "at least one target must be configured".to_string(),
            ));
        }

        let mut seen_paths = std::collections::HashSet::new();
        for target in &self.targets {
            if !target.path.is_absolute() {
                return Err(ConfigError::Invalid(format!(
                    "target path must be absolute: {}",
                    target.path.display()
                )));
            }

            if !seen_paths.insert(&target.path) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate target path: {}",
                    target.path.display()
                )));
            }

            if target.interval_seconds == 0 {
                return Err(ConfigError::Invalid(format!(
                    "target {} interval_seconds must be > 0",
                    target.name()
                )));
            }
        }

        Ok(())
    }

    /// Generate a default configuration
    pub fn default_config_toml() -> String {
        r#"# gsd - git snapshot daemon configuration
schema_version = "1"

[logging]
level = "info"
# directory = "/var/log/gsd"
console = true

[git]
author_name = "gsd"
author_email = "gsd@local"
default_ignore_patterns = ["*.db-wal", "*.db-shm", "*.db-journal"]

# Example target configuration
# [[targets]]
# path = "/home/user/notes"
# interval_seconds = 60
# ignore_patterns = ["*.tmp"]
# enabled = true

# You can also create a .gsdignore file in any target directory
# for target-specific excludes (like .gitignore syntax)
"#
        .to_string()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            logging: LoggingConfig::default(),
            git: GitConfig::default(),
            targets: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
            [[targets]]
            path = "/tmp/test"
        "#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.targets.len(), 1);
        assert_eq!(config.targets[0].path, PathBuf::from("/tmp/test"));
        assert_eq!(config.targets[0].interval_seconds, DEFAULT_INTERVAL_SECONDS);
    }

    #[test]
    fn test_validate_duplicate_paths() {
        let config = Config {
            targets: vec![
                TargetConfig {
                    path: PathBuf::from("/tmp/same"),
                    interval_seconds: 60,
                    ignore_patterns: vec![],
                    enabled: true,
                },
                TargetConfig {
                    path: PathBuf::from("/tmp/same"),
                    interval_seconds: 60,
                    ignore_patterns: vec![],
                    enabled: true,
                },
            ],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_relative_path() {
        let config = Config {
            targets: vec![TargetConfig {
                path: PathBuf::from("relative/path"),
                interval_seconds: 60,
                ignore_patterns: vec![],
                enabled: true,
            }],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }
}
