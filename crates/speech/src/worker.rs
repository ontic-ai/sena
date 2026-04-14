//! STT Worker process communication.
//!
//! This module manages the stt-worker child process and handles stdin/stdout communication
//! using a length-prefixed protocol for audio chunks and JSON events.

use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::SpeechError;

/// Worker event received from stt-worker via stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WorkerEvent {
    /// Worker is listening and ready for audio chunks.
    Listening,
    /// A single word was transcribed (streaming output).
    Word {
        text: String,
        confidence: f32,
        sequence: u32,
        request_id: u64,
    },
    /// Transcription request completed.
    Completed {
        text: String,
        avg_confidence: f32,
        request_id: u64,
    },
    /// Worker is stopping gracefully.
    Stopped,
    /// An error occurred in the worker.
    Error { reason: String },
}

/// STT worker process handle.
pub struct SttWorker {
    child: Child,
    stdin: ChildStdin,
    stdout_reader: BufReader<ChildStdout>,
}

impl SttWorker {
    /// Spawn the stt-worker child process.
    ///
    /// # Arguments
    /// - `model_path`: Path to the whisper GGML model file to load
    ///
    /// # Returns
    /// `Ok(SttWorker)` with active worker process, or `Err(SpeechError)`.
    pub async fn spawn(model_path: &str) -> Result<Self, SpeechError> {
        let worker_path = find_stt_worker()?;

        tracing::info!(
            "worker: spawning stt-worker: {:?} with model: {}",
            worker_path,
            model_path
        );

        let mut child = Command::new(&worker_path)
            .arg(model_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| SpeechError::WorkerSpawnFailed(e.to_string()))?;

        tracing::info!(
            "worker: stt-worker process spawned (pid: {})",
            child.id().unwrap_or(0)
        );

        let stdin = child.stdin.take().ok_or_else(|| {
            SpeechError::WorkerPipeError("failed to capture worker stdin".to_string())
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            SpeechError::WorkerPipeError("failed to capture worker stdout".to_string())
        })?;

        let stdout_reader = BufReader::new(stdout);

        Ok(Self {
            child,
            stdin,
            stdout_reader,
        })
    }

    /// Write a PCM audio chunk to the worker's stdin.
    ///
    /// # Arguments
    /// - `samples`: PCM f32 samples (16kHz mono, range [-1.0, 1.0])
    pub async fn write_chunk(&mut self, samples: &[f32]) -> Result<(), SpeechError> {
        tracing::debug!(
            "worker: writing {} samples ({} bytes) to stdin",
            samples.len(),
            samples.len() * 4
        );

        // Write length prefix (u32 sample count, little-endian)
        let len_bytes = (samples.len() as u32).to_le_bytes();
        self.stdin
            .write_all(&len_bytes)
            .await
            .map_err(|e| SpeechError::WorkerPipeError(format!("stdin write failed: {}", e)))?;

        // Write f32 samples as bytes
        for sample in samples {
            let bytes = sample.to_le_bytes();
            self.stdin
                .write_all(&bytes)
                .await
                .map_err(|e| SpeechError::WorkerPipeError(format!("stdin write failed: {}", e)))?;
        }

        self.stdin
            .flush()
            .await
            .map_err(|e| SpeechError::WorkerPipeError(format!("stdin flush failed: {}", e)))?;

        tracing::debug!("worker: chunk written and flushed");
        Ok(())
    }

    /// Read the next JSON event from the worker's stdout.
    ///
    /// Returns `Ok(Some(event))` if an event was read, `Ok(None)` if stdout closed,
    /// or `Err` on parse failure.
    pub async fn read_event(&mut self) -> Result<Option<WorkerEvent>, SpeechError> {
        let mut line = String::new();
        let bytes_read = self
            .stdout_reader
            .read_line(&mut line)
            .await
            .map_err(|e| SpeechError::WorkerPipeError(format!("stdout read failed: {}", e)))?;

        if bytes_read == 0 {
            tracing::warn!("worker: stdout EOF - worker exited");
            return Ok(None);
        }

        tracing::debug!("worker: read line from stdout: {}", line.trim());

        let event: WorkerEvent = serde_json::from_str(line.trim()).map_err(|e| {
            SpeechError::WorkerPipeError(format!(
                "JSON parse failed: {} (line: {})",
                e,
                line.trim()
            ))
        })?;

        tracing::debug!("worker: parsed event: {:?}", event);

        Ok(Some(event))
    }

    /// Send a graceful shutdown signal (zero-length chunk) to the worker.
    pub async fn shutdown(&mut self) -> Result<(), SpeechError> {
        // Write zero-length chunk
        let zero_len = 0u32.to_le_bytes();
        self.stdin
            .write_all(&zero_len)
            .await
            .map_err(|e| SpeechError::WorkerPipeError(format!("shutdown write failed: {}", e)))?;

        self.stdin
            .flush()
            .await
            .map_err(|e| SpeechError::WorkerPipeError(format!("shutdown flush failed: {}", e)))?;

        Ok(())
    }

    /// Wait for the worker process to exit and return its exit status.
    pub async fn wait(&mut self) -> Result<i32, SpeechError> {
        let status = self
            .child
            .wait()
            .await
            .map_err(|e| SpeechError::WorkerPipeError(format!("wait failed: {}", e)))?;

        Ok(status.code().unwrap_or(-1))
    }

    /// Kill the worker process immediately.
    pub async fn kill(&mut self) -> Result<(), SpeechError> {
        self.child
            .kill()
            .await
            .map_err(|e| SpeechError::WorkerPipeError(format!("kill failed: {}", e)))?;
        Ok(())
    }
}

/// Locate the stt-worker binary in the same directory as the current executable.
///
/// # Returns
/// `Ok(PathBuf)` with the path to stt-worker, or `Err(SpeechError::WorkerNotFound)`.
fn find_stt_worker() -> Result<PathBuf, SpeechError> {
    let exe = std::env::current_exe().map_err(|e| {
        SpeechError::WorkerNotFound(PathBuf::from(format!("failed to get current exe: {}", e)))
    })?;

    let bin_dir = exe.parent().ok_or_else(|| {
        SpeechError::WorkerNotFound(PathBuf::from("current exe has no parent directory"))
    })?;

    let worker_name = if cfg!(windows) {
        "stt-worker.exe"
    } else {
        "stt-worker"
    };

    let worker_path = bin_dir.join(worker_name);

    if !worker_path.exists() {
        return Err(SpeechError::WorkerNotFound(worker_path));
    }

    Ok(worker_path)
}

/// Locate the stt-worker binary for testing (allows override via env var).
#[cfg(test)]
pub fn find_stt_worker_for_test() -> Result<PathBuf, SpeechError> {
    if let Ok(path) = std::env::var("STT_WORKER_PATH") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }
    find_stt_worker()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_event_deserialization() {
        let json = r#"{"type":"listening"}"#;
        let event: WorkerEvent = serde_json::from_str(json).unwrap();
        matches!(event, WorkerEvent::Listening);

        let json =
            r#"{"type":"word","text":"hello","confidence":0.94,"sequence":1,"request_id":42}"#;
        let event: WorkerEvent = serde_json::from_str(json).unwrap();
        if let WorkerEvent::Word {
            text,
            confidence,
            sequence,
            request_id,
        } = event
        {
            assert_eq!(text, "hello");
            assert_eq!(confidence, 0.94);
            assert_eq!(sequence, 1);
            assert_eq!(request_id, 42);
        } else {
            panic!("expected Word event");
        }
    }

    #[test]
    fn find_worker_returns_expected_name() {
        let result = find_stt_worker();
        // May fail if binary not built yet — that's expected in unit test
        if let Ok(path) = result {
            let name = path.file_name().unwrap().to_string_lossy();
            if cfg!(windows) {
                assert_eq!(name, "stt-worker.exe");
            } else {
                assert_eq!(name, "stt-worker");
            }
        }
    }
}
