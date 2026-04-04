//! Config field validation.
//!
//! All validators are pure functions — no I/O, no bus calls.
//! Returns Err(ValidationError) with a descriptive human-readable reason.

use std::fmt::Display;
use std::str::FromStr;

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid value for '{field}': {reason}")]
    IllegalValue { field: String, reason: String },
}

/// Validate a config key-value pair before applying it.
///
/// Returns `Ok(())` if the value is valid for the given key.
/// Returns `Err(ValidationError)` if the value is invalid.
///
/// Unknown keys return `Ok(())` — the deserializer will reject them later if needed.
pub fn validate_config_set(key: &str, value: &str) -> Result<(), ValidationError> {
    match key {
        "inference_max_tokens" => {
            parse_in_range::<u32>("inference_max_tokens", value, 128, 65536)?;
        }
        "ctp_trigger_interval_secs" => {
            parse_in_range::<f64>("ctp_trigger_interval_secs", value, 0.5, 3600.0)?;
        }
        "shutdown_timeout_secs" => {
            parse_in_range::<u64>("shutdown_timeout_secs", value, 1, 300)?;
        }
        "stt_energy_threshold" => {
            parse_in_range::<f32>("stt_energy_threshold", value, 0.001, 1.0)?;
        }
        "silence_duration_secs" => {
            parse_in_range::<f32>("silence_duration_secs", value, 0.1, 30.0)?;
        }
        "tts_rate" => {
            parse_in_range::<f32>("tts_rate", value, 0.1, 4.0)?;
        }
        "wakeword_sensitivity" => {
            parse_in_range::<f32>("wakeword_sensitivity", value, 0.0, 1.0)?;
        }
        "speech_enabled" | "voice_always_listening" | "wakeword_enabled" => {
            parse_bool(key, value)?;
        }
        "proactive_output_mode" => {
            validate_proactive_output_mode(value)?;
        }
        _ => {
            // Unknown keys pass validation — let the deserializer reject them if needed
        }
    }
    Ok(())
}

/// Parse and validate a value is within an inclusive range.
fn parse_in_range<T>(field: &str, value: &str, min: T, max: T) -> Result<T, ValidationError>
where
    T: FromStr + PartialOrd + Display,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| ValidationError::IllegalValue {
            field: field.to_string(),
            reason: format!("expected {}, got '{}'", std::any::type_name::<T>(), value),
        })?;

    if parsed < min || parsed > max {
        return Err(ValidationError::IllegalValue {
            field: field.to_string(),
            reason: format!("must be between {} and {}, got {}", min, max, parsed),
        });
    }

    Ok(parsed)
}

/// Parse and validate a boolean value.
fn parse_bool(field: &str, value: &str) -> Result<(), ValidationError> {
    if value != "true" && value != "false" {
        return Err(ValidationError::IllegalValue {
            field: field.to_string(),
            reason: format!("expected 'true' or 'false', got '{}'", value),
        });
    }
    Ok(())
}

/// Validate proactive_output_mode is one of the allowed values.
fn validate_proactive_output_mode(value: &str) -> Result<(), ValidationError> {
    match value {
        "none" | "tts" | "tray" | "both" => Ok(()),
        _ => Err(ValidationError::IllegalValue {
            field: "proactive_output_mode".to_string(),
            reason: format!(
                "must be one of: 'none', 'tts', 'tray', 'both', got '{}'",
                value
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_inference_max_tokens_valid() {
        assert!(validate_config_set("inference_max_tokens", "2048").is_ok());
    }

    #[test]
    fn validate_inference_max_tokens_too_small() {
        let result = validate_config_set("inference_max_tokens", "50");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("must be between 128 and 65536"));
    }

    #[test]
    fn validate_inference_max_tokens_too_large() {
        let result = validate_config_set("inference_max_tokens", "100000");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("must be between 128 and 65536"));
    }

    #[test]
    fn validate_stt_energy_threshold_valid() {
        assert!(validate_config_set("stt_energy_threshold", "0.02").is_ok());
    }

    #[test]
    fn validate_stt_energy_threshold_invalid() {
        let result = validate_config_set("stt_energy_threshold", "2.0");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("must be between 0.001 and 1"));
    }

    #[test]
    fn validate_bool_valid() {
        assert!(validate_config_set("speech_enabled", "true").is_ok());
        assert!(validate_config_set("speech_enabled", "false").is_ok());
    }

    #[test]
    fn validate_bool_invalid() {
        let result = validate_config_set("speech_enabled", "maybe");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("expected 'true' or 'false'"));
    }

    #[test]
    fn validate_unknown_key_passes() {
        assert!(validate_config_set("some_future_key", "anything").is_ok());
    }

    #[test]
    fn validate_proactive_output_mode_valid() {
        assert!(validate_config_set("proactive_output_mode", "tts").is_ok());
        assert!(validate_config_set("proactive_output_mode", "none").is_ok());
        assert!(validate_config_set("proactive_output_mode", "tray").is_ok());
        assert!(validate_config_set("proactive_output_mode", "both").is_ok());
    }

    #[test]
    fn validate_proactive_output_mode_invalid() {
        let result = validate_config_set("proactive_output_mode", "loud");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("must be one of"));
    }
}
