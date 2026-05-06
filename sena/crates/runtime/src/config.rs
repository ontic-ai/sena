//! Configuration system for Sena (nested workspace).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Configuration for Sena runtime and subsystems.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SenaConfig {
    /// File paths to watch for changes.
    #[serde(default)]
    pub file_watch_paths: Vec<PathBuf>,

    /// Whether clipboard observation is enabled.
    #[serde(default = "default_clipboard_observation_enabled")]
    pub clipboard_observation_enabled: bool,

    /// Whether speech actors should be spawned at boot.
    #[serde(default = "default_speech_enabled")]
    pub speech_enabled: bool,

    /// Whether boot should request daemon listen mode after all actors are ready.
    ///
    /// Defaults to enabled so speech-first routing is active at boot.
    #[serde(default = "default_always_listen")]
    pub always_listen: bool,

    /// Whether wakeword detection is active.
    ///
    /// This remains disabled by default because the current placeholder
    /// detector is not suitable for always-on production use unless the user
    /// explicitly opts in.
    #[serde(default = "default_wakeword_enabled")]
    pub wakeword_enabled: bool,

    /// Wakeword detection sensitivity [0.0, 1.0].
    #[serde(default = "default_wakeword_sensitivity")]
    pub wakeword_sensitivity: f32,

    /// Maximum number of tokens to generate per inference response.
    #[serde(default = "default_inference_max_tokens")]
    pub inference_max_tokens: usize,

    /// Whether local token-usage auto-tuning is enabled.
    #[serde(default = "default_auto_tune_tokens")]
    pub auto_tune_tokens: bool,

    /// Minimum token budget the auto-tuner may select.
    #[serde(default = "default_auto_tune_min_tokens")]
    pub auto_tune_min_tokens: usize,

    /// Maximum token budget the auto-tuner may select.
    #[serde(default = "default_auto_tune_max_tokens")]
    pub auto_tune_max_tokens: usize,
}

impl Default for SenaConfig {
    fn default() -> Self {
        Self {
            file_watch_paths: Vec::new(),
            clipboard_observation_enabled: default_clipboard_observation_enabled(),
            speech_enabled: default_speech_enabled(),
            always_listen: default_always_listen(),
            wakeword_enabled: default_wakeword_enabled(),
            wakeword_sensitivity: default_wakeword_sensitivity(),
            inference_max_tokens: default_inference_max_tokens(),
            auto_tune_tokens: default_auto_tune_tokens(),
            auto_tune_min_tokens: default_auto_tune_min_tokens(),
            auto_tune_max_tokens: default_auto_tune_max_tokens(),
        }
    }
}

fn default_clipboard_observation_enabled() -> bool {
    true
}

fn default_speech_enabled() -> bool {
    true
}

fn default_always_listen() -> bool {
    true
}

fn default_wakeword_enabled() -> bool {
    false
}

fn default_wakeword_sensitivity() -> f32 {
    0.5
}

fn default_inference_max_tokens() -> usize {
    512
}

fn default_auto_tune_tokens() -> bool {
    true
}

fn default_auto_tune_min_tokens() -> usize {
    256
}

fn default_auto_tune_max_tokens() -> usize {
    4096
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

/// Load config from disk, or create it with defaults when missing.
pub async fn load_or_create_config() -> Result<SenaConfig, ConfigError> {
    let path = config_path()?;
    load_or_create_config_at(&path).await
}

/// Save config to disk.
pub async fn save_config(config: &SenaConfig) -> Result<(), ConfigError> {
    let path = config_path()?;
    save_config_at(config, &path).await
}

/// Apply a scalar config change and persist it.
pub async fn apply_config_set(key: &str, value: &str) -> Result<(), String> {
    let mut config = load_or_create_config()
        .await
        .map_err(|e| format!("failed to load config: {}", e))?;

    apply_config_value(&mut config, key, value)?;
    save_config(&config)
        .await
        .map_err(|e| format!("failed to save config: {}", e))?;
    Ok(())
}

pub(crate) async fn load_or_create_config_at(path: &Path) -> Result<SenaConfig, ConfigError> {
    if tokio::fs::metadata(path).await.is_ok() {
        let contents = tokio::fs::read_to_string(path).await?;
        let config: SenaConfig = toml::from_str(&contents)?;
        Ok(config)
    } else {
        let config = SenaConfig::default();
        save_config_at(&config, path).await?;
        Ok(config)
    }
}

async fn save_config_at(config: &SenaConfig, path: &Path) -> Result<(), ConfigError> {
    let toml_string = toml::to_string_pretty(config)?;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(path, toml_string).await?;
    Ok(())
}

fn apply_config_value(config: &mut SenaConfig, key: &str, value: &str) -> Result<(), String> {
    match key {
        "file_watch_paths" => {
            config.file_watch_paths = value
                .split([';', ',', '\n'])
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .map(PathBuf::from)
                .collect();
        }
        "clipboard_observation_enabled" => {
            config.clipboard_observation_enabled = value
                .parse::<bool>()
                .map_err(|_| "expected true or false".to_string())?;
        }
        "speech_enabled" => {
            config.speech_enabled = value
                .parse::<bool>()
                .map_err(|_| "expected true or false".to_string())?;
        }
        "always_listen" => {
            config.always_listen = value
                .parse::<bool>()
                .map_err(|_| "expected true or false".to_string())?;
        }
        "wakeword_enabled" => {
            config.wakeword_enabled = value
                .parse::<bool>()
                .map_err(|_| "expected true or false".to_string())?;
        }
        "wakeword_sensitivity" => {
            let parsed = value
                .parse::<f32>()
                .map_err(|_| "expected a decimal between 0.0 and 1.0".to_string())?;
            if !(0.0..=1.0).contains(&parsed) {
                return Err("wakeword_sensitivity must be between 0.0 and 1.0".to_string());
            }
            config.wakeword_sensitivity = parsed;
        }
        "inference_max_tokens" => {
            config.inference_max_tokens = value
                .parse::<usize>()
                .map_err(|_| "expected a positive integer".to_string())?;
        }
        "auto_tune_tokens" => {
            config.auto_tune_tokens = value
                .parse::<bool>()
                .map_err(|_| "expected true or false".to_string())?;
        }
        "auto_tune_min_tokens" => {
            config.auto_tune_min_tokens = value
                .parse::<usize>()
                .map_err(|_| "expected a positive integer".to_string())?;
        }
        "auto_tune_max_tokens" => {
            config.auto_tune_max_tokens = value
                .parse::<usize>()
                .map_err(|_| "expected a positive integer".to_string())?;
        }
        _ => {
            return Err(format!(
                "unknown key '{}'. Supported keys: clipboard_observation_enabled, speech_enabled, always_listen, wakeword_enabled, wakeword_sensitivity, inference_max_tokens, auto_tune_tokens, auto_tune_min_tokens, auto_tune_max_tokens",
                key
            ));
        }
    }

    if config.auto_tune_min_tokens > config.auto_tune_max_tokens {
        return Err("auto_tune_min_tokens cannot exceed auto_tune_max_tokens".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config_has_expected_values() {
        let config = SenaConfig::default();
        assert!(config.file_watch_paths.is_empty());
        assert!(config.clipboard_observation_enabled);
        assert!(config.speech_enabled);
        assert!(config.always_listen);
        assert!(!config.wakeword_enabled);
        assert_eq!(config.wakeword_sensitivity, 0.5);
        assert_eq!(config.inference_max_tokens, 512);
        assert!(config.auto_tune_tokens);
        assert_eq!(config.auto_tune_min_tokens, 256);
        assert_eq!(config.auto_tune_max_tokens, 4096);
    }

    #[tokio::test]
    async fn load_or_create_config_creates_default_when_missing() {
        let dir = tempdir().expect("failed to create tempdir");
        let config_path = dir.path().join("config.toml");

        let config = load_or_create_config_at(&config_path)
            .await
            .expect("load_or_create should succeed");

        assert!(config_path.exists());
        assert_eq!(config, SenaConfig::default());
    }

    #[tokio::test]
    async fn load_or_create_config_loads_existing() {
        let dir = tempdir().expect("failed to create tempdir");
        let config_path = dir.path().join("config.toml");
        let custom_config = SenaConfig {
            clipboard_observation_enabled: false,
            speech_enabled: false,
            always_listen: true,
            wakeword_enabled: true,
            wakeword_sensitivity: 0.75,
            inference_max_tokens: 1024,
            auto_tune_tokens: false,
            auto_tune_min_tokens: 300,
            auto_tune_max_tokens: 2048,
            ..Default::default()
        };
        save_config_at(&custom_config, &config_path)
            .await
            .expect("save should succeed");

        let loaded = load_or_create_config_at(&config_path)
            .await
            .expect("load should succeed");

        assert_eq!(loaded, custom_config);
    }

    #[test]
    fn apply_config_value_updates_scalar_fields() {
        let mut config = SenaConfig::default();

        apply_config_value(&mut config, "file_watch_paths", "C:/one;C:/two")
            .expect("file_watch_paths should parse");
        apply_config_value(&mut config, "speech_enabled", "false")
            .expect("speech_enabled should parse");
        apply_config_value(&mut config, "always_listen", "true")
            .expect("always_listen should parse");
        apply_config_value(&mut config, "wakeword_enabled", "true")
            .expect("wakeword_enabled should parse");
        apply_config_value(&mut config, "wakeword_sensitivity", "0.7")
            .expect("wakeword_sensitivity should parse");
        apply_config_value(&mut config, "inference_max_tokens", "768")
            .expect("inference_max_tokens should parse");
        apply_config_value(&mut config, "auto_tune_tokens", "false")
            .expect("auto_tune_tokens should parse");

        assert_eq!(
            config.file_watch_paths,
            vec![PathBuf::from("C:/one"), PathBuf::from("C:/two")]
        );
        assert!(!config.speech_enabled);
        assert!(config.always_listen);
        assert!(config.wakeword_enabled);
        assert_eq!(config.wakeword_sensitivity, 0.7);
        assert_eq!(config.inference_max_tokens, 768);
        assert!(!config.auto_tune_tokens);
    }

    #[test]
    fn apply_config_value_rejects_invalid_wakeword_sensitivity() {
        let mut config = SenaConfig::default();
        let result = apply_config_value(&mut config, "wakeword_sensitivity", "1.5");
        assert!(result.is_err());
    }

    #[test]
    fn apply_config_value_rejects_invalid_auto_tune_range() {
        let mut config = SenaConfig::default();
        apply_config_value(&mut config, "auto_tune_max_tokens", "512")
            .expect("auto_tune_max_tokens should parse");

        let result = apply_config_value(&mut config, "auto_tune_min_tokens", "1024");
        assert!(result.is_err());
    }
}
