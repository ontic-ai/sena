//! Speech-related IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};

/// Handler for "speech.listen_start" command.
pub struct SpeechListenStartHandler;

#[async_trait]
impl CommandHandler for SpeechListenStartHandler {
    fn name(&self) -> &'static str {
        "speech.listen_start"
    }

    fn description(&self) -> &'static str {
        "Start speech listening (STT capture)"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        // Phase 2 limitation: speech loop control via bus events not yet wired.
        Err(IpcError::CommandNotReady(
            "Speech listen start not yet implemented".to_string(),
        ))
    }
}

/// Handler for "speech.listen_stop" command.
pub struct SpeechListenStopHandler;

#[async_trait]
impl CommandHandler for SpeechListenStopHandler {
    fn name(&self) -> &'static str {
        "speech.listen_stop"
    }

    fn description(&self) -> &'static str {
        "Stop speech listening (STT capture)"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        // Phase 2 limitation: speech loop control via bus events not yet wired.
        Err(IpcError::CommandNotReady(
            "Speech listen stop not yet implemented".to_string(),
        ))
    }
}

/// Handler for "speech.status" command.
pub struct SpeechStatusHandler;

#[async_trait]
impl CommandHandler for SpeechStatusHandler {
    fn name(&self) -> &'static str {
        "speech.status"
    }

    fn description(&self) -> &'static str {
        "Get speech subsystem status"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        // Phase 2 limitation: no runtime helper for speech actor status.
        Ok(json!({
            "stt_enabled": false,
            "tts_enabled": false,
            "note": "Speech status query not yet implemented"
        }))
    }
}
