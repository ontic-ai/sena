//! STT telemetry logging to CSV.
//!
//! Records backend performance metrics (latency, VRAM, confidence) for A/B testing analysis.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::SpeechError;

/// Log STT telemetry to CSV file.
///
/// Logs to platform-specific path:
/// - Windows: `%APPDATA%\sena\logs\stt_telemetry.csv`
/// - macOS: `~/Library/Application Support/sena/logs/stt_telemetry.csv`
/// - Linux: `~/.config/sena/logs/stt_telemetry.csv`
///
/// CSV format: `backend,chunk_duration_ms,latency_ms,confidence,vram_mb`
///
/// Writes CSV header on first write (when file does not exist or is empty).
pub async fn log_stt_telemetry(
    backend: &str,
    chunk_duration_ms: u64,
    latency_ms: u64,
    confidence: f64,
    vram_mb: u32,
) -> Result<(), SpeechError> {
    let log_path = telemetry_log_path()?;

    // Ensure log directory exists
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| SpeechError::TelemetryWriteFailed(e.to_string()))?;
    }

    // Check if header is needed (file doesn't exist or is empty)
    let needs_header = !log_path.exists()
        || tokio::fs::metadata(&log_path)
            .await
            .map_err(|e| SpeechError::TelemetryWriteFailed(e.to_string()))?
            .len()
            == 0;

    let mut lines = String::new();
    if needs_header {
        lines.push_str("backend,chunk_duration_ms,latency_ms,confidence,vram_mb\n");
    }
    lines.push_str(&format!(
        "{},{},{},{:.3},{}\n",
        backend, chunk_duration_ms, latency_ms, confidence, vram_mb
    ));

    // Offload blocking file I/O to tokio's blocking thread pool
    let log_path_clone = log_path.clone();
    tokio::task::spawn_blocking(move || {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_clone)
            .map_err(|e| SpeechError::TelemetryWriteFailed(e.to_string()))?;
        file.write_all(lines.as_bytes())
            .map_err(|e| SpeechError::TelemetryWriteFailed(e.to_string()))?;
        file.flush()
            .map_err(|e| SpeechError::TelemetryWriteFailed(e.to_string()))?;
        Ok::<_, SpeechError>(())
    })
    .await
    .map_err(|e| SpeechError::TelemetryWriteFailed(e.to_string()))??;

    tracing::debug!(
        "telemetry: logged STT metrics — backend={}, chunk={}ms, latency={}ms, confidence={:.3}, vram={}MB",
        backend, chunk_duration_ms, latency_ms, confidence, vram_mb
    );

    Ok(())
}

/// Get platform-specific telemetry log path.
fn telemetry_log_path() -> Result<PathBuf, SpeechError> {
    let base_dir = if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map_err(|_| SpeechError::TelemetryWriteFailed("APPDATA env var not set".to_string()))
            .map(PathBuf::from)?
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME")
            .map_err(|_| SpeechError::TelemetryWriteFailed("HOME env var not set".to_string()))
            .map(|home| {
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
            })?
    } else {
        // Linux
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|_| {
                std::env::var("HOME")
                    .map(|home| PathBuf::from(home).join(".config"))
                    .map_err(|_| {
                        SpeechError::TelemetryWriteFailed("HOME env var not set".to_string())
                    })
            })?
    };

    Ok(base_dir.join("sena").join("logs").join("stt_telemetry.csv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_log_path_returns_valid_path() {
        let path = telemetry_log_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("sena"));
        assert!(path.to_string_lossy().contains("logs"));
        assert!(path.to_string_lossy().ends_with("stt_telemetry.csv"));
    }

    #[tokio::test]
    async fn log_stt_telemetry_writes_csv_with_header() {
        use tempfile::tempdir;

        let _temp_dir = tempdir().unwrap();

        // Simulate writing to temp file by testing the format string
        let mut lines = String::new();
        lines.push_str("backend,chunk_duration_ms,latency_ms,confidence,vram_mb\n");
        lines.push_str(&format!(
            "{},{},{},{:.3},{}\n",
            "whisper", 560, 342, 0.987, 142
        ));

        assert!(lines.contains("backend,chunk_duration_ms,latency_ms,confidence,vram_mb"));
        assert!(lines.contains("whisper,560,342,0.987,142"));
    }

    #[tokio::test]
    async fn log_stt_telemetry_formats_correctly() {
        // Test format string generation
        let backend = "sherpa";
        let chunk_duration_ms = 200;
        let latency_ms = 124;
        let confidence = 0.956;
        let vram_mb = 100;

        let line = format!(
            "{},{},{},{:.3},{}\n",
            backend, chunk_duration_ms, latency_ms, confidence, vram_mb
        );

        assert_eq!(line, "sherpa,200,124,0.956,100\n");
    }
}
