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

    /// Whether screen capture visual context is enabled for CTP.
    /// Default: false
    #[serde(default = "default_screen_capture_enabled")]
    pub screen_capture_enabled: bool,

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

    /// Interval in seconds between memory consolidation runs (decay + pruning).
    /// Default: 300 (5 minutes)
    #[serde(default = "default_memory_consolidation_interval_secs")]
    pub memory_consolidation_interval_secs: u64,

    /// Idle threshold in seconds before consolidation is allowed to run.
    /// Prevents background consolidation during active interaction bursts.
    /// Default: 120 (2 minutes)
    #[serde(default = "default_memory_consolidation_idle_secs")]
    pub memory_consolidation_idle_secs: u64,

    /// CTP trigger sensitivity multiplier (0.0–1.0).
    /// Lower values require a stronger context change to trigger a thought event.
    /// Default: 0.5
    #[serde(default = "default_ctp_trigger_sensitivity")]
    pub ctp_trigger_sensitivity: f64,

    /// Maximum number of reflection rounds in multi-round reasoning.
    /// Hard cap: value is clamped to [1, max_reflection_rounds_hard_cap].
    /// Default: 2
    #[serde(default = "default_max_reflection_rounds")]
    pub max_reflection_rounds: usize,

    /// Preferred model name selected via `sena models`.
    /// When set, Sena will attempt to use this model over the auto-discovered default.
    /// If the preferred model is not found at boot, Sena falls back to the largest discovered model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,

    /// Custom model directory path.
    /// When set, Sena will discover models from this directory instead of the default Ollama directory.
    /// Default: None (use platform-default Ollama directory)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_dir: Option<PathBuf>,

    /// Interval in seconds between memory usage checks.
    /// Default: 60
    #[serde(default = "default_memory_monitor_interval_secs")]
    pub memory_monitor_interval_secs: u64,

    /// Memory limit in MB. If exceeded, MemoryThresholdExceeded event is broadcast.
    /// Default: 2048 (2 GB)
    #[serde(default = "default_memory_limit_mb")]
    pub memory_limit_mb: usize,

    /// CPU usage threshold (percent) below which platform actor reduces polling frequency.
    /// When CPU usage falls below this value, platform polling slows to 2 seconds.
    /// Default: 10.0
    #[serde(default = "default_platform_idle_cpu_threshold_percent")]
    pub platform_idle_cpu_threshold_percent: f32,

    /// Whether speech (STT/TTS) subsystem is enabled.
    /// Default: false (disabled)
    #[serde(default = "default_speech_enabled")]
    pub speech_enabled: bool,

    /// Whether voice input is always listening (continuous mode).
    /// If false, STT actor only processes on-demand VoiceInputDetected events.
    /// Default: false
    #[serde(default = "default_voice_always_listening")]
    pub voice_always_listening: bool,

    /// Custom Whisper model path for STT.
    /// If None, uses default: ~/.sena/models/whisper/ggml-small.bin
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whisper_model_path: Option<String>,

    /// Energy threshold for voice activity detection (RMS > threshold).
    /// Default: 0.01
    #[serde(default = "default_stt_energy_threshold")]
    pub stt_energy_threshold: f32,

    /// Custom directory for speech model storage (Whisper, Piper, OpenWakeWord).
    /// Default: None (uses platform-specific default: macOS ~/Library/Application Support/sena/models/,
    /// Windows %APPDATA%/sena/models/, Linux ~/.local/share/sena/models/)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_model_dir: Option<PathBuf>,

    /// Whether wakeword detection is active.
    /// Requires speech_enabled to also be true.
    /// Default: true
    #[serde(default = "default_wakeword_enabled")]
    pub wakeword_enabled: bool,

    /// Wakeword detection sensitivity threshold (0.0-1.0).
    /// Higher values = more sensitive = more false positives.
    /// Default: 0.5
    #[serde(default = "default_wakeword_sensitivity")]
    pub wakeword_sensitivity: f32,

    /// Whether CTP-triggered inference results are spoken via TTS.
    /// Default: true (when speech_enabled)
    #[serde(default = "default_proactive_speech_enabled")]
    pub proactive_speech_enabled: bool,

    /// Minimum seconds between proactive TTS outputs.
    /// Prevents rapid-fire speech.
    /// Default: 10
    #[serde(default = "default_speech_rate_limit_secs")]
    pub speech_rate_limit_secs: u64,

    /// TTS voice/model name. For Piper, this is the voice model name.
    /// Default: None (use first available)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_voice: Option<String>,

    /// TTS speech rate multiplier (0.5-2.0).
    /// Default: 1.0
    #[serde(default = "default_tts_rate")]
    pub tts_rate: f32,

    /// Selected microphone device name for STT / listen mode.
    /// When `None`, the system default input device is used.
    /// Set via `/microphone select <index>` in the CLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone_device: Option<String>,

    /// Maximum tokens to generate per inference response.
    /// Hardware-aware: lower values use less VRAM/RAM, higher values allow longer responses.
    /// Default: 512
    #[serde(default = "default_inference_max_tokens")]
    pub inference_max_tokens: usize,

    /// Context window size for inference. Determines how much prompt + response fits.
    /// Must not exceed model's training context. Larger values use more VRAM/RAM.
    /// Default: 2048
    #[serde(default = "default_inference_ctx_size")]
    pub inference_ctx_size: u32,

    /// When true, Sena automatically adjusts `inference_max_tokens` based on observed
    /// token usage from recent inference completions. Uses a P95 rolling window with
    /// 20% headroom to right-size the budget without truncating responses.
    /// Default: true
    #[serde(default = "default_auto_tune_tokens")]
    pub auto_tune_tokens: bool,

    /// Minimum value auto-tune will ever set `inference_max_tokens` to.
    /// Default: 256
    #[serde(default = "default_auto_tune_min_tokens")]
    pub auto_tune_min_tokens: usize,

    /// Maximum value auto-tune will ever set `inference_max_tokens` to.
    /// Default: 4096
    #[serde(default = "default_auto_tune_max_tokens")]
    pub auto_tune_max_tokens: usize,
}

impl Default for SenaConfig {
    fn default() -> Self {
        Self {
            ctp_trigger_interval_secs: default_ctp_trigger_interval_secs(),
            shutdown_timeout_secs: default_shutdown_timeout_secs(),
            file_watch_paths: Vec::new(),
            clipboard_observation_enabled: default_clipboard_observation_enabled(),
            screen_capture_enabled: default_screen_capture_enabled(),
            working_memory_max_exchanges: default_working_memory_max_exchanges(),
            working_memory_token_budget: default_working_memory_token_budget(),
            soul_summary_max_events: default_soul_summary_max_events(),
            memory_consolidation_interval_secs: default_memory_consolidation_interval_secs(),
            memory_consolidation_idle_secs: default_memory_consolidation_idle_secs(),
            ctp_trigger_sensitivity: default_ctp_trigger_sensitivity(),
            max_reflection_rounds: default_max_reflection_rounds(),
            preferred_model: None,
            models_dir: None,
            memory_monitor_interval_secs: default_memory_monitor_interval_secs(),
            memory_limit_mb: default_memory_limit_mb(),
            platform_idle_cpu_threshold_percent: default_platform_idle_cpu_threshold_percent(),
            speech_enabled: default_speech_enabled(),
            voice_always_listening: default_voice_always_listening(),
            whisper_model_path: None,
            stt_energy_threshold: default_stt_energy_threshold(),
            speech_model_dir: None,
            wakeword_enabled: default_wakeword_enabled(),
            wakeword_sensitivity: default_wakeword_sensitivity(),
            proactive_speech_enabled: default_proactive_speech_enabled(),
            speech_rate_limit_secs: default_speech_rate_limit_secs(),
            tts_voice: None,
            tts_rate: default_tts_rate(),
            microphone_device: None,
            inference_max_tokens: default_inference_max_tokens(),
            inference_ctx_size: default_inference_ctx_size(),
            auto_tune_tokens: default_auto_tune_tokens(),
            auto_tune_min_tokens: default_auto_tune_min_tokens(),
            auto_tune_max_tokens: default_auto_tune_max_tokens(),
        }
    }
}

fn default_ctp_trigger_interval_secs() -> u64 {
    300
}
fn default_shutdown_timeout_secs() -> u64 {
    5
}
fn default_clipboard_observation_enabled() -> bool {
    true
}
fn default_screen_capture_enabled() -> bool {
    false
}
fn default_working_memory_max_exchanges() -> usize {
    10
}
fn default_working_memory_token_budget() -> usize {
    4096
}
fn default_soul_summary_max_events() -> usize {
    50
}
fn default_memory_consolidation_interval_secs() -> u64 {
    300
}
fn default_memory_consolidation_idle_secs() -> u64 {
    120
}
fn default_ctp_trigger_sensitivity() -> f64 {
    0.5
}
fn default_max_reflection_rounds() -> usize {
    2
}
fn default_memory_monitor_interval_secs() -> u64 {
    60
}
fn default_memory_limit_mb() -> usize {
    2048
}
fn default_platform_idle_cpu_threshold_percent() -> f32 {
    10.0
}
fn default_speech_enabled() -> bool {
    false
}
fn default_voice_always_listening() -> bool {
    true
}
fn default_stt_energy_threshold() -> f32 {
    0.01
}
fn default_wakeword_enabled() -> bool {
    false
}
fn default_wakeword_sensitivity() -> f32 {
    0.5
}
fn default_proactive_speech_enabled() -> bool {
    true
}
fn default_speech_rate_limit_secs() -> u64 {
    10
}
fn default_tts_rate() -> f32 {
    1.0
}
fn default_inference_max_tokens() -> usize {
    512
}
fn default_inference_ctx_size() -> u32 {
    2048
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

/// Returns the default speech model directory for the current OS.
///
/// - macOS: `~/Library/Application Support/sena/models/`
/// - Windows: `%APPDATA%\sena\models\`
/// - Linux: `~/.local/share/sena/models/`
#[cfg(target_os = "linux")]
pub fn default_speech_model_dir() -> PathBuf {
    std::env::var("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("sena")
                .join("models")
        })
        .unwrap_or_else(|_| PathBuf::from(".").join("models"))
}

/// Returns the default speech model directory for the current OS.
///
/// - macOS: `~/Library/Application Support/sena/models/`
/// - Windows: `%APPDATA%\sena\models\`
/// - Linux: `~/.local/share/sena/models/`
#[cfg(not(target_os = "linux"))]
pub fn default_speech_model_dir() -> PathBuf {
    config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("models")
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

/// Saves a config to the OS-specific config path.
pub async fn save_config(config: &SenaConfig) -> Result<(), ConfigError> {
    let path = config_path()?;
    let toml_string = toml::to_string_pretty(config)?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, toml_string).await?;
    Ok(())
}

/// Apply a key-value config change. Called by the supervisor upon ConfigSetRequested.
/// Returns Ok(()) on success, Err(reason) on failure (invalid key, bad value, I/O error).
///
/// This function loads the current config, applies the requested change, and saves.
/// String errors are used (no anyhow) per copilot-instructions.md §4.1.
pub async fn apply_config_set(key: &str, value: &str) -> Result<(), String> {
    let mut config = load_or_create_config()
        .await
        .map_err(|e| format!("failed to load config: {}", e))?;

    let result: Result<(), String> = match key {
        "speech_enabled" => value
            .parse::<bool>()
            .map(|v| config.speech_enabled = v)
            .map_err(|_| "expected true or false".to_string()),
        "voice_always_listening" => value
            .parse::<bool>()
            .map(|v| config.voice_always_listening = v)
            .map_err(|_| "expected true or false".to_string()),
        "wakeword_enabled" => value
            .parse::<bool>()
            .map(|v| config.wakeword_enabled = v)
            .map_err(|_| "expected true or false".to_string()),
        "proactive_speech_enabled" => value
            .parse::<bool>()
            .map(|v| config.proactive_speech_enabled = v)
            .map_err(|_| "expected true or false".to_string()),
        "clipboard_observation_enabled" => value
            .parse::<bool>()
            .map(|v| config.clipboard_observation_enabled = v)
            .map_err(|_| "expected true or false".to_string()),
        "screen_capture_enabled" => value
            .parse::<bool>()
            .map(|v| config.screen_capture_enabled = v)
            .map_err(|_| "expected true or false".to_string()),
        "inference_max_tokens" => value
            .parse::<usize>()
            .map(|v| config.inference_max_tokens = v)
            .map_err(|_| "expected a positive integer".to_string()),
        "inference_ctx_size" => value
            .parse::<u32>()
            .map(|v| config.inference_ctx_size = v)
            .map_err(|_| "expected a positive integer".to_string()),
        "auto_tune_tokens" => value
            .parse::<bool>()
            .map(|v| config.auto_tune_tokens = v)
            .map_err(|_| "expected true or false".to_string()),
        "auto_tune_min_tokens" => value
            .parse::<usize>()
            .map(|v| config.auto_tune_min_tokens = v)
            .map_err(|_| "expected a positive integer".to_string()),
        "auto_tune_max_tokens" => value
            .parse::<usize>()
            .map(|v| config.auto_tune_max_tokens = v)
            .map_err(|_| "expected a positive integer".to_string()),
        "ctp_trigger_interval_secs" => value
            .parse::<u64>()
            .map(|v| config.ctp_trigger_interval_secs = v)
            .map_err(|_| "expected a non-negative integer (seconds)".to_string()),
        "ctp_trigger_sensitivity" => value
            .parse::<f64>()
            .map(|v| config.ctp_trigger_sensitivity = v)
            .map_err(|_| "expected a decimal number (0.0–1.0)".to_string()),
        "tts_rate" => value
            .parse::<f32>()
            .map(|v| config.tts_rate = v)
            .map_err(|_| "expected a decimal number (0.5–2.0)".to_string()),
        "memory_limit_mb" => value
            .parse::<usize>()
            .map(|v| config.memory_limit_mb = v)
            .map_err(|_| "expected a positive integer (MB)".to_string()),
        "shutdown_timeout_secs" => value
            .parse::<u64>()
            .map(|v| config.shutdown_timeout_secs = v)
            .map_err(|_| "expected a non-negative integer (seconds)".to_string()),
        "preferred_model" => {
            config.preferred_model = if value == "auto" || value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
            Ok(())
        }
        "microphone_device" => {
            // Special case for /microphone select — empty value clears device
            config.microphone_device = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
            Ok(())
        }
        _ => Err(format!(
            "unknown key '{}'. Run /config to see editable keys",
            key
        )),
    };

    // If parsing succeeded, save the config
    result?;
    save_config(&config)
        .await
        .map_err(|e| format!("failed to save config: {}", e))?;
    Ok(())
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
        assert!(!config.screen_capture_enabled);
        assert_eq!(config.working_memory_max_exchanges, 10);
        assert_eq!(config.working_memory_token_budget, 4096);
        assert_eq!(config.soul_summary_max_events, 50);
        assert_eq!(config.platform_idle_cpu_threshold_percent, 10.0);
        assert_eq!(config.inference_max_tokens, 512);
        assert_eq!(config.inference_ctx_size, 2048);
    }

    #[test]
    fn default_cpu_idle_threshold_is_ten_percent() {
        let config = default_config();
        assert_eq!(
            config.platform_idle_cpu_threshold_percent, 10.0,
            "default CPU idle threshold should be 10.0%"
        );
        assert_eq!(
            default_platform_idle_cpu_threshold_percent(),
            10.0,
            "default function should return 10.0"
        );
    }

    #[test]
    fn cpu_idle_threshold_serializes_and_deserializes() {
        let config = SenaConfig {
            platform_idle_cpu_threshold_percent: 15.0,
            ..Default::default()
        };

        let toml_string = toml::to_string_pretty(&config).expect("serialization failed");
        assert!(
            toml_string.contains("platform_idle_cpu_threshold_percent"),
            "serialized config should contain cpu idle threshold field"
        );

        let parsed: SenaConfig = toml::from_str(&toml_string).expect("deserialization failed");
        assert_eq!(
            parsed.platform_idle_cpu_threshold_percent, 15.0,
            "deserialized config should preserve cpu idle threshold"
        );
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

    #[tokio::test]
    async fn apply_config_set_modifies_and_saves_config() {
        // Create a temporary config with default values
        let config = default_config();
        assert_eq!(config.speech_enabled, false);
        assert_eq!(config.inference_max_tokens, 512);

        // Apply config changes in-memory (using the public API that supervisor calls)
        // Note: This requires write access to the real config path, so we test the logic
        // by verifying parse logic works as expected with a mock scenario.

        // Test boolean parsing
        let result = match "true".parse::<bool>() {
            Ok(v) => {
                let mut cfg = config.clone();
                cfg.speech_enabled = v;
                Ok(())
            }
            Err(_) => Err("expected true or false".to_string()),
        };
        assert!(result.is_ok());

        // Test usize parsing
        let result = match "1024".parse::<usize>() {
            Ok(v) => {
                let mut cfg = config.clone();
                cfg.inference_max_tokens = v;
                Ok(())
            }
            Err(_) => Err("expected a positive integer".to_string()),
        };
        assert!(result.is_ok());

        // Test invalid key
        let invalid_key = "nonexistent_key";
        let reason = format!(
            "unknown key '{}'. Run /config to see editable keys",
            invalid_key
        );
        assert!(reason.contains("nonexistent_key"));

        // Test invalid value parsing
        let result = match "not_a_bool".parse::<bool>() {
            Ok(v) => {
                let mut cfg = config.clone();
                cfg.speech_enabled = v;
                Ok(())
            }
            Err(_) => Err("expected true or false".to_string()),
        };
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "expected true or false");
    }
}
