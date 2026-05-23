//! Speech-related IPC command handlers.

use crate::commands::runtime_commands::RuntimeState;
use async_trait::async_trait;
use bus::{CausalId, Event, EventBus, SpeechEvent, SystemEvent};
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::sync::Arc;

/// Handler for "speech.listen_start" command.
pub struct SpeechListenStartHandler {
    bus: Arc<EventBus>,
    state: RuntimeState,
}

impl SpeechListenStartHandler {
    pub fn new(bus: Arc<EventBus>, state: RuntimeState) -> Self {
        Self { bus, state }
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
        self.state.ensure_actors_running(&["stt"]).await?;

        let causal_id = CausalId::new();

        self.bus
            .broadcast(Event::System(SystemEvent::LoopControlRequested {
                loop_name: "speech".to_string(),
                enabled: true,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        self.bus
            .broadcast(Event::Speech(SpeechEvent::ListenModeRequested {
                causal_id,
            }))
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
    state: RuntimeState,
}

impl SpeechListenStopHandler {
    pub fn new(bus: Arc<EventBus>, state: RuntimeState) -> Self {
        Self { bus, state }
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
        self.state.ensure_actors_running(&["stt"]).await?;

        let causal_id = CausalId::new();
        let mut rx = self.bus.subscribe_broadcast();

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

        let wait_result = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                match rx.recv().await {
                    Ok(Event::Speech(SpeechEvent::ListenModeStopped {
                        causal_id: event_causal_id,
                        transcript,
                    })) if event_causal_id == causal_id => {
                        if let Some(raw_transcript) = transcript {
                            self.bus
                                .broadcast(Event::Speech(
                                    SpeechEvent::ListenModeTranscriptFinalized {
                                        text: raw_transcript.clone(),
                                        causal_id,
                                    },
                                ))
                                .await
                                .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

                            return Ok(Some(raw_transcript));
                        }

                        return Ok(None);
                    }
                    Ok(_) => {}
                    Err(e) => return Err(IpcError::CommandFailed(e.to_string())),
                }
            }
        })
        .await;

        let finalized_text = match wait_result {
            Ok(result) => result?,
            Err(_) => None,
        };

        Ok(json!({
            "status": "requested",
            "listening": false,
            "causal_id": causal_id.as_u64(),
            "finalized_text": finalized_text,
        }))
    }
}

/// Handler for "speech.say" command.
pub struct SpeechSayHandler {
    bus: Arc<EventBus>,
    state: RuntimeState,
}

impl SpeechSayHandler {
    pub fn new(bus: Arc<EventBus>, state: RuntimeState) -> Self {
        Self { bus, state }
    }
}

#[async_trait]
impl CommandHandler for SpeechSayHandler {
    fn name(&self) -> &'static str {
        "speech.say"
    }

    fn description(&self) -> &'static str {
        "Speak text verbatim through TTS"
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        self.state.ensure_actors_running(&["tts"]).await?;

        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                IpcError::InvalidPayload("missing or invalid 'text' field".to_string())
            })?;

        let causal_id = CausalId::new();

        self.bus
            .broadcast(Event::Speech(SpeechEvent::SpeakRequested {
                text: text.to_string(),
                causal_id,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        Ok(json!({
            "status": "requested",
            "text_chars": text.chars().count(),
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
            "wakeword_enabled": status.wakeword_enabled,
            "wakeword_ready": status.wakeword_ready,
            "stt_backend": status.stt_backend,
            "speech_models_dir": status.speech_models_dir,
            "mode": if status.speech_enabled { "enabled" } else { "disabled" },
        }))
    }
}
