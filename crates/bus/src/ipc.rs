//! IPC protocol types for CLI ↔ daemon communication.
//!
//! Phase 6: CLI becomes a separate process that communicates with the daemon
//! over a Unix socket (macOS/Linux) or named pipe (Windows). All messages
//! are JSON-over-newline-delimited format.

use serde::{Deserialize, Serialize};

/// IPC protocol schema version. Increment on breaking payload changes.
pub const IPC_SCHEMA_VERSION: u8 = 1;

/// Top-level envelope for all IPC messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcMessage {
    /// Request ID. 0 = daemon push (no request).
    pub id: u64,
    /// Message payload.
    pub payload: IpcPayload,
}

/// All message variants sent in either direction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "d")]
pub enum IpcPayload {
    // ── CLI → Daemon ─────────────────────────────────────────────────────────
    /// First message a CLI client sends: register for event stream.
    Subscribe,

    /// User typed text in the chat input.
    Chat { text: String },

    /// User executed a slash command (full line including the leading slash).
    SlashCommand { line: String },

    /// Keepalive.
    Ping,

    // ── Daemon → CLI ─────────────────────────────────────────────────────────
    /// Command acknowledged (sent back with the same id as the request).
    Ack { to_id: u64 },

    /// A line of content to display in the CLI.
    DisplayLine { content: String, style: LineStyle },

    /// Protocol error (invalid command, handler failed, etc.).
    Error { to_id: u64, reason: String },

    /// Response to Ping.
    Pong,

    /// Daemon is ready and the session is established.
    /// Carries the schema version so the CLI can detect protocol mismatches.
    /// Also carries the currently active model name (if one has been loaded).
    SessionReady {
        schema_version: u8,
        #[serde(default)]
        current_model: Option<String>,
    },

    /// Daemon → CLI: the active inference model has changed.
    /// Sent whenever `InferenceEvent::ModelLoaded` fires so the CLI sidebar stays current.
    ModelStatusUpdate { name: String },

    /// Daemon is shutting down — CLI should disconnect cleanly.
    DaemonShutdown,

    /// Daemon → CLI: a background loop's enabled state has changed.
    /// Sent to all connected clients immediately when loop state changes and as an
    /// initial sync burst (one message per loop) when a client subscribes.
    LoopStatusUpdate {
        /// Canonical loop name matching §17.2 of copilot-instructions (e.g. `"ctp"`).
        loop_name: String,
        /// `true` = loop is running, `false` = paused/disabled.
        enabled: bool,
    },

    /// CLI → Daemon: request graceful shutdown.
    /// Only sent by a CLI instance that auto-started the daemon (tracked by `cli_started_daemon`).
    /// Must NOT be sent when connecting to a pre-existing daemon.
    ShutdownRequested,

    /// CLI → Daemon: initialize Soul with user name (first-boot onboarding).
    /// Sent by CLI after collecting onboarding data but before daemon was running.
    /// Daemon broadcasts `SoulEvent::InitializeWithName` on the bus.
    InitializeName { name: String },
}

/// Display hint for CLI rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineStyle {
    /// Normal white text.
    Normal,
    /// Dimmed/gray — partial results, hints, not finalized.
    Dimmed,
    /// Red — errors.
    Error,
    /// Italic/yellow — CTP proactive thoughts.
    CtpThought,
    /// Cyan — system notices (boot, config, actor status).
    SystemNotice,
    /// Green — success/completion.
    Success,
    /// Magenta — inference output.
    Inference,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_subscribe_serializes() {
        let msg = IpcMessage {
            id: 1,
            payload: IpcPayload::Subscribe,
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.id, 1);
        assert!(matches!(parsed.payload, IpcPayload::Subscribe));
    }

    #[test]
    fn ipc_display_line_serializes() {
        let msg = IpcMessage {
            id: 0,
            payload: IpcPayload::DisplayLine {
                content: "test output".to_string(),
                style: LineStyle::Normal,
            },
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.id, 0);
        match parsed.payload {
            IpcPayload::DisplayLine { content, style } => {
                assert_eq!(content, "test output");
                assert_eq!(style, LineStyle::Normal);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ipc_error_serializes() {
        let msg = IpcMessage {
            id: 42,
            payload: IpcPayload::Error {
                to_id: 5,
                reason: "command failed".to_string(),
            },
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.id, 42);
        match parsed.payload {
            IpcPayload::Error { to_id, reason } => {
                assert_eq!(to_id, 5);
                assert_eq!(reason, "command failed");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ipc_all_line_styles_serialize() {
        let styles = vec![
            LineStyle::Normal,
            LineStyle::Dimmed,
            LineStyle::Error,
            LineStyle::CtpThought,
            LineStyle::SystemNotice,
            LineStyle::Success,
            LineStyle::Inference,
        ];

        for style in styles {
            let msg = IpcMessage {
                id: 0,
                payload: IpcPayload::DisplayLine {
                    content: "test".to_string(),
                    style,
                },
            };

            let json = serde_json::to_string(&msg).expect("serialize");
            let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

            match parsed.payload {
                IpcPayload::DisplayLine {
                    style: parsed_style,
                    ..
                } => {
                    assert_eq!(parsed_style, style);
                }
                _ => panic!("wrong variant"),
            }
        }
    }

    #[test]
    fn ipc_chat_serializes() {
        let msg = IpcMessage {
            id: 10,
            payload: IpcPayload::Chat {
                text: "hello sena".to_string(),
            },
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.id, 10);
        match parsed.payload {
            IpcPayload::Chat { text } => {
                assert_eq!(text, "hello sena");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ipc_slash_command_serializes() {
        let msg = IpcMessage {
            id: 11,
            payload: IpcPayload::SlashCommand {
                line: "/model swap".to_string(),
            },
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.id, 11);
        match parsed.payload {
            IpcPayload::SlashCommand { line } => {
                assert_eq!(line, "/model swap");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ipc_ping_pong_serialize() {
        let ping = IpcMessage {
            id: 99,
            payload: IpcPayload::Ping,
        };
        let pong = IpcMessage {
            id: 99,
            payload: IpcPayload::Pong,
        };

        let ping_json = serde_json::to_string(&ping).expect("serialize ping");
        let pong_json = serde_json::to_string(&pong).expect("serialize pong");

        let parsed_ping: IpcMessage = serde_json::from_str(&ping_json).expect("deserialize ping");
        let parsed_pong: IpcMessage = serde_json::from_str(&pong_json).expect("deserialize pong");

        assert!(matches!(parsed_ping.payload, IpcPayload::Ping));
        assert!(matches!(parsed_pong.payload, IpcPayload::Pong));
    }

    #[test]
    fn ipc_session_ready_serializes() {
        let msg = IpcMessage {
            id: 0,
            payload: IpcPayload::SessionReady {
                schema_version: IPC_SCHEMA_VERSION,
                current_model: None,
            },
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert!(matches!(parsed.payload, IpcPayload::SessionReady { .. }));
    }

    #[test]
    fn ipc_session_ready_carries_schema_version() {
        let msg = IpcMessage {
            id: 0,
            payload: IpcPayload::SessionReady {
                schema_version: IPC_SCHEMA_VERSION,
                current_model: None,
            },
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");
        match parsed.payload {
            IpcPayload::SessionReady { schema_version, .. } => {
                assert_eq!(schema_version, IPC_SCHEMA_VERSION);
            }
            _ => panic!("expected SessionReady"),
        }
    }

    #[test]
    fn ipc_daemon_shutdown_serializes() {
        let msg = IpcMessage {
            id: 0,
            payload: IpcPayload::DaemonShutdown,
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert!(matches!(parsed.payload, IpcPayload::DaemonShutdown));
    }

    #[test]
    fn ipc_ack_serializes() {
        let msg = IpcMessage {
            id: 100,
            payload: IpcPayload::Ack { to_id: 50 },
        };

        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.id, 100);
        match parsed.payload {
            IpcPayload::Ack { to_id } => {
                assert_eq!(to_id, 50);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ipc_loop_status_update_serializes() {
        let msg = IpcMessage {
            id: 0,
            payload: IpcPayload::LoopStatusUpdate {
                loop_name: "ctp".to_string(),
                enabled: true,
            },
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");
        match parsed.payload {
            IpcPayload::LoopStatusUpdate { loop_name, enabled } => {
                assert_eq!(loop_name, "ctp");
                assert!(enabled);
            }
            _ => panic!("expected LoopStatusUpdate"),
        }
    }

    #[test]
    fn ipc_shutdown_requested_serializes() {
        let msg = IpcMessage {
            id: 0,
            payload: IpcPayload::ShutdownRequested,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let parsed: IpcMessage = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(parsed.payload, IpcPayload::ShutdownRequested));
    }
}
