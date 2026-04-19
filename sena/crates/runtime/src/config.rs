//! Configuration system for Sena (nested workspace).
//!
//! Minimal implementation to support onboarding config persistence.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Configuration for Sena runtime and subsystems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenaConfig {
    /// File paths to watch for changes.
    #[serde(default)]
    pub file_watch_paths: Vec<PathBuf>,

    /// Whether clipboard observation is enabled.
    #[serde(default = "default_clipboard_observation_enabled")]
    pub clipboard_observation_enabled: bool,
}

impl Default for SenaConfig {
    fn default() -> Self {
        Self {
            file_watch_paths: Vec::new(),
            clipboard_observation_enabled: true,
        }
    }
}

fn default_clipboard_observation_enabled() -> bool {
    true
}

/// Configuration errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("Directory resolution failed: {0}")]
    DirectoryResolution(String),
}

/// Resolve the Sena config directory.
fn config_dir() -> Result<PathBuf, ConfigError> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("sena"))
            .map_err(|e| ConfigError::DirectoryResolution(format!("APPDATA not set: {}", e)))
    }

    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME")
            .map(|home| {
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("sena")
            })
            .map_err(|e| ConfigError::DirectoryResolution(format!("HOME not set: {}", e)))
    }

    #[cfg(target_os = "linux")]
    {
        std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".config").join("sena"))
            .map_err(|e| ConfigError::DirectoryResolution(format!("HOME not set: {}", e)))
    }
}

/// Get the config file path.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

/// Load config from disk, or create default if missing.
pub async fn load_or_create_config() -> Result<SenaConfig, ConfigError> {
    let path = config_path()?;

    if tokio::fs::metadata(&path).await.is_ok() {
        let content = tokio::fs::read_to_string(&path).await?;
        let config: SenaConfig = toml::from_str(&content)?;
        Ok(config)
    } else {
        Ok(SenaConfig::default())
    }
}

/// Save config to disk.
pub async fn save_config(config: &SenaConfig) -> Result<(), ConfigError> {
    let path = config_path()?;
    let toml_string = toml::to_string_pretty(config)?;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(path, toml_string).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn config_loads_default() {
        let config = SenaConfig::default();
        assert!(config.file_watch_paths.is_empty());
        assert!(config.clipboard_observation_enabled);
    }
}
