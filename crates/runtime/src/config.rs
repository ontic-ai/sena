//! Configuration system for Sena.
//!
//! Handles loading and creating config files from OS-appropriate locations.
//! Config is loaded at boot step 1 (see architecture.md Â§4.1).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration for Sena runtime and subsystems.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SenaConfig {
    /// Interval in seconds between CTP thought trigger evaluations.
    /// Default: 300 (5 minutes)
    #[serde(default = "default_ctp_trigger_interval_secs")]
    pub ctp_trigger_interval_secs: u64,

    /// Timeout in seconds for graceful shutdown of each actor.
    /// Default: 5 seconds
    #[serde(default = "default_shutdown_timeout_secs")]
    pub shutdown_timeout_secs: u64,

    /// File paths to watch for changes. Platform adapter will monitor these.
    /// Default: empty (no file watching)
    #[serde(default)]
    pub file_watch_paths: Vec<PathBuf>,

    /// Whether clipboard observation is enabled.
    /// Default: true
    #[serde(default = "default_clipboard_observation_enabled")]
    pub clipboard_observation_enabled: bool,

    /// Maximum number of inference exchanges kept in working memory per cycle.
    /// Default: 10
    #[serde(default = "default_working_memory_max_exchanges")]
    pub working_memory_max_exchanges: usize,

    /// Token budget for working memory exchanges.
    /// Oldest exchanges are evicted when budget is exceeded.
    /// Default: 4096
    #[serde(default = "default_working_memory_token_budget")]
    pub working_memory_token_budget: usize,

    /// Maximum number of recent Soul events included in the prompt summary.
    /// Default: 50
    #[serde(default = "default_soul_summary_max_events")]
    pub soul_summary_max_events: usize,
}

impl Default for SenaConfig {
    fn default() -> Self {
        Self {
            ctp_trigger_interval_secs: default_ctp_trigger_interval_secs(),
            shutdown_timeout_secs: default_shutdown_timeout_secs(),
            file_watch_paths: Vec::new(),
            clipboard_observation_enabled: default_clipboard_observation_enabled(),
            working_memory_max_exchanges: default_working_memory_max_exchanges(),
            working_memory_token_budget: default_working_memory_token_budget(),
            soul_summary_max_events: default_soul_summary_max_events(),
        }
    }
}

fn default_ctp_trigger_interval_secs() -> u64 { 300 }
fn default_shutdown_timeout_secs() -> u64 { 5 }
fn default_clipboard_observation_enabled() -> bool { true }
fn default_working_memory_max_exchanges() -> usize { 10 }
fn default_working_memory_token_budget() -> usize { 4096 }
fn default_soul_summary_max_events() -> usize { 50 }

/// Configuration-related errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("config serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("config directory unavailable: {0}")]
    ConfigDirUnavailable(String),
}

/// Returns the OS-specific config directory for Sena.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    platform::config_dir().map_err(|e| ConfigError::ConfigDirUnavailable(e.to_string()))
}

/// Returns the full path to the config file.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn default_config() -> SenaConfig {
    SenaConfig::default()
}

/// Loads config from the OS-specific config path, or creates it with defaults if missing.
pub async fn load_or_create_config() -> Result<SenaConfig, ConfigError> {
    let path = config_path()?;
    load_or_create_config_at(&path).await
}

pub(crate) async fn load_or_create_config_at(
    path: &std::path::Path,
) -> Result<SenaConfig, ConfigError> {
    if tokio::fs::metadata(path).await.is_ok() {
        let contents = tokio::fs::read_to_string(path).await?;
        let config: SenaConfig = toml::from_str(&contents)?;
        Ok(config)
    } else {
        let config = default_config();
        let toml_string = toml::to_string_pretty(&config)?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(path, toml_string).await?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config_has_expected_values() {
        let config = default_config();
        assert_eq!(config.ctp_trigger_interval_secs, 300);
        assert_eq!(config.shutdown_timeout_secs, 5);
        assert!(config.file_watch_paths.is_empty());
        assert!(config.clipboard_observation_enabled);
        assert_eq!(config.working_memory_max_exchanges, 10);
        assert_eq!(config.working_memory_token_budget, 4096);
        assert_eq!(config.soul_summary_max_events, 50);
    }

    #[test]
    fn default_config_serialization_round_trip() {
        let config = default_config();
        let toml_string = toml::to_string_pretty(&config).expect("serialization failed");
        let parsed: SenaConfig = toml::from_str(&toml_string).expect("deserialization failed");
        assert_eq!(config, parsed);
    }

    #[tokio::test]
    async fn load_or_create_config_creates_default_when_missing() {
        let dir = tempdir().expect("failed to create tempdir");
        let config_path = dir.path().join("config.toml");

        assert!(!config_path.exists());

        let config = load_or_create_config_at(&config_path)
            .await
            .expect("load_or_create failed");

        assert!(config_path.exists());
        assert_eq!(config, default_config());

        let contents = tokio::fs::read_to_string(&config_path)
            .await
            .expect("failed to read config file");
        let parsed: SenaConfig = toml::from_str(&contents).expect("failed to parse written file");
        assert_eq!(parsed, default_config());
    }

    #[tokio::test]
    async fn load_or_create_config_loads_existing() {
        let dir = tempdir().expect("failed to create tempdir");
        let config_path = dir.path().join("config.toml");

        // Use ..Default::default() to forward-compat with future field additions.
        let custom_config = SenaConfig {
            ctp_trigger_interval_secs: 600,
            shutdown_timeout_secs: 10,
            file_watch_paths: vec![PathBuf::from("/tmp/test")],
            clipboard_observation_enabled: false,
            ..Default::default()
        };
        let toml_string = toml::to_string_pretty(&custom_config).expect("serialization failed");
        tokio::fs::write(&config_path, toml_string)
            .await
            .expect("failed to write config");

        let loaded = load_or_create_config_at(&config_path)
            .await
            .expect("load_or_create failed");

        assert_eq!(loaded, custom_config);
    }

    #[tokio::test]
    async fn corrupted_toml_returns_parse_error() {
        let dir = tempdir().expect("failed to create tempdir");
        let config_path = dir.path().join("config.toml");

        tokio::fs::write(&config_path, "this is not valid toml {{{")
            .await
            .expect("failed to write invalid toml");

        let result = load_or_create_config_at(&config_path).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(ConfigError::Parse(_))));
    }
}