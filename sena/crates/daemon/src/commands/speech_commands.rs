//! Speech-related IPC command handlers.

use async_trait::async_trait;
use bus::{CausalId, Event, EventBus, SpeechEvent, SystemEvent};
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::sync::Arc;

/// Handler for "speech.listen_start" command.
pub struct SpeechListenStartHandler {
    bus: Arc<EventBus>,
}

impl SpeechListenStartHandler {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for SpeechListenStartHandler {
    fn name(&self) -> &'static str {
        "speech.listen_start"
    }

    fn description(&self) -> &'static str {
        "Start speech listening (STT capture)"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let causal_id = CausalId::new();

        self.bus
            .broadcast(Event::System(SystemEvent::LoopControlRequested {
                loop_name: "speech".to_string(),
                enabled: true,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        self.bus
            .broadcast(Event::Speech(SpeechEvent::ListenModeRequested { causal_id }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        Ok(json!({
            "status": "requested",
            "listening": true,
            "causal_id": causal_id.as_u64(),
        }))
    }
}

/// Handler for "speech.listen_stop" command.
pub struct SpeechListenStopHandler {
    bus: Arc<EventBus>,
}

impl SpeechListenStopHandler {
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }
}

#[async_trait]
impl CommandHandler for SpeechListenStopHandler {
    fn name(&self) -> &'static str {
        "speech.listen_stop"
    }

    fn description(&self) -> &'static str {
        "Stop speech listening (STT capture)"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let causal_id = CausalId::new();

        self.bus
            .broadcast(Event::Speech(SpeechEvent::ListenModeStopRequested {
                causal_id,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        self.bus
            .broadcast(Event::System(SystemEvent::LoopControlRequested {
                loop_name: "speech".to_string(),
                enabled: false,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        Ok(json!({
            "status": "requested",
            "listening": false,
            "causal_id": causal_id.as_u64(),
        }))
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
        let status = runtime::speech_status_snapshot()
            .await
            .map_err(IpcError::CommandFailed)?;

        Ok(json!({
            "speech_enabled": status.speech_enabled,
            "stt_enabled": status.stt_enabled,
            "tts_enabled": status.tts_enabled,
            "stt_backend": status.stt_backend,
            "speech_models_dir": status.speech_models_dir,
            "mode": if status.speech_enabled { "enabled" } else { "disabled" },
        }))
    }
}
