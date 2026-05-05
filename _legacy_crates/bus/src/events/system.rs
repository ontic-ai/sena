//! System-level events: boot, shutdown, failure.

/// Information about a failed actor.
#[derive(Debug, Clone)]
pub struct ActorFailureInfo {
    /// Static name of the actor that failed.
    pub actor_name: &'static str,
    /// Error message describing the failure.
    pub error_msg: String,
}

/// Items in the system tray context menu.
#[derive(Debug, Clone, PartialEq)]
pub enum TrayMenuItem {
    /// Show session status (uptime, messages sent).
    ShowStatus,
    /// Show last thought summary.
    ShowLastThought,
    /// Open CLI in a new terminal window.
    OpenCli,
    /// Open the log folder in the system file manager.
    ViewLogs,
    /// Quit Sena.
    Quit,
}

/// System lifecycle and control events.
#[derive(Debug, Clone)]
pub enum SystemEvent {
    /// Signal to initiate graceful shutdown.
    ShutdownSignal,
    /// Boot sequence completed successfully.
    BootComplete,
    /// Emitted when this is the first time Sena has been run (no prior Soul database found).
    FirstBoot,
    /// An actor has failed.
    ActorFailed(ActorFailureInfo),
    /// Emitted by each actor when it has successfully started and is ready.
    ActorReady { actor_name: &'static str },
    /// Encryption subsystem initialized successfully.
    EncryptionInitialized,
    /// User clicked a tray menu item.
    TrayMenuClicked(TrayMenuItem),
    /// Tray initialized successfully.
    TrayReady,
    /// Tray initialization failed (non-fatal — Sena continues without tray).
    TrayUnavailable { reason: String },
    /// Memory usage exceeded configured threshold.
    MemoryThresholdExceeded { current_mb: usize, limit_mb: usize },
    /// Request to open/attach an interactive CLI session (e.g., from tray menu).
    CliAttachRequested,
    /// CLI session has closed without requesting app shutdown.
    CliSessionClosed,
    /// Soul database could not be decrypted and was backed up before recovery.
    DatabaseRecovered {
        /// Path to the backup file created.
        backup_path: String,
    },
    /// Request runtime to reload configuration from disk (hot-reload).
    /// No actors are restarted — only config values are refreshed.
    ///
    /// TODO M6: wire to file-watch notification or IPC command.
    ConfigReloadRequested,
    /// Configuration was successfully reloaded.
    ConfigReloaded,
    /// Request from CLI/UI to set a config key-value pair.
    /// The supervisor handles validation, persistence, and broadcasts ConfigReloaded on success.
    ConfigSetRequested {
        /// The config key to set (e.g. "speech_enabled", "inference_max_tokens").
        key: String,
        /// The string value to parse and apply (e.g. "true", "512").
        value: String,
    },
    /// A config set request failed (invalid key, invalid value, I/O error).
    ConfigSetFailed {
        /// The config key that failed to set.
        key: String,
        /// Error reason (e.g. "unknown key", "expected integer").
        reason: String,
    },
    /// inference_max_tokens was automatically adjusted based on observed usage.
    TokenBudgetAutoTuned {
        /// Previous token limit.
        old_max_tokens: usize,
        /// New token limit after tuning.
        new_max_tokens: usize,
        /// P95 token count from the observation window that drove this decision.
        p95_tokens: usize,
    },
    /// Request to pause or resume a named background loop.
    /// Dispatched by the IPC server when the CLI sends `/loops <name>` or `/loops <name> on|off`.
    /// The actor owning the named loop must handle this event and broadcast `LoopStatusChanged`.
    LoopControlRequested {
        /// Canonical loop name (lowercase, underscore-separated). See §17.2 in copilot-instructions.
        loop_name: String,
        /// `true` = enable the loop, `false` = disable/pause it.
        enabled: bool,
    },
    /// Broadcast by an actor when its loop's enabled state changes.
    /// The IPC server listens for this and forwards `IpcPayload::LoopStatusUpdate` to all
    /// connected CLI clients so the sidebar updates in real time.
    LoopStatusChanged {
        /// Canonical loop name. Same namespace as `LoopControlRequested`.
        loop_name: String,
        /// Current state after the change.
        enabled: bool,
    },
    /// Real-time VRAM usage telemetry from the vram_monitor background loop.
    VramUsageUpdated {
        /// Total GPU VRAM in megabytes. 0 if no GPU detected.
        total_mb: u64,
        /// Currently used VRAM in megabytes.
        used_mb: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_signal_constructs_and_clones() {
        let event = SystemEvent::ShutdownSignal;
        let cloned = event.clone();
        matches!(cloned, SystemEvent::ShutdownSignal);
    }

    #[test]
    fn boot_complete_constructs_and_clones() {
        let event = SystemEvent::BootComplete;
        let cloned = event.clone();
        matches!(cloned, SystemEvent::BootComplete);
    }

    #[test]
    fn actor_failed_constructs_and_clones() {
        let info = ActorFailureInfo {
            actor_name: "test_actor",
            error_msg: "test error".to_string(),
        };
        let event = SystemEvent::ActorFailed(info);
        let cloned = event.clone();

        if let SystemEvent::ActorFailed(failure_info) = cloned {
            assert_eq!(failure_info.actor_name, "test_actor");
            assert_eq!(failure_info.error_msg, "test error");
        } else {
            panic!("Expected ActorFailed variant");
        }
    }

    #[test]
    fn actor_failure_info_clones_independently() {
        let info = ActorFailureInfo {
            actor_name: "actor1",
            error_msg: "error1".to_string(),
        };
        let cloned = info.clone();
        assert_eq!(cloned.actor_name, "actor1");
        assert_eq!(cloned.error_msg, "error1");
    }

    #[test]
    fn actor_ready_constructs_and_clones() {
        let event = SystemEvent::ActorReady {
            actor_name: "Platform",
        };
        let cloned = event.clone();
        if let SystemEvent::ActorReady { actor_name } = cloned {
            assert_eq!(actor_name, "Platform");
        } else {
            panic!("Expected ActorReady variant");
        }
    }

    #[test]
    fn memory_threshold_exceeded_constructs_and_clones() {
        let event = SystemEvent::MemoryThresholdExceeded {
            current_mb: 2500,
            limit_mb: 2048,
        };
        let cloned = event.clone();
        if let SystemEvent::MemoryThresholdExceeded {
            current_mb,
            limit_mb,
        } = cloned
        {
            assert_eq!(current_mb, 2500);
            assert_eq!(limit_mb, 2048);
        } else {
            panic!("Expected MemoryThresholdExceeded variant");
        }
    }

    // Compile-time verification: SystemEvent and ActorFailureInfo are Send
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}

    #[test]
    fn types_are_send() {
        assert_send::<SystemEvent>();
        assert_send::<ActorFailureInfo>();
    }

    #[test]
    fn vram_usage_updated_constructs_and_clones() {
        let event = SystemEvent::VramUsageUpdated {
            total_mb: 8192,
            used_mb: 3200,
        };
        let cloned = event.clone();
        if let SystemEvent::VramUsageUpdated { total_mb, used_mb } = cloned {
            assert_eq!(total_mb, 8192);
            assert_eq!(used_mb, 3200);
        } else {
            panic!("Expected VramUsageUpdated variant");
        }
    }

    #[test]
    fn loop_control_requested_constructs_and_clones() {
        let event = SystemEvent::LoopControlRequested {
            loop_name: "ctp".to_string(),
            enabled: false,
        };
        let cloned = event.clone();
        if let SystemEvent::LoopControlRequested { loop_name, enabled } = cloned {
            assert_eq!(loop_name, "ctp");
            assert!(!enabled);
        } else {
            panic!("expected LoopControlRequested");
        }
    }

    #[test]
    fn loop_status_changed_constructs_and_clones() {
        let event = SystemEvent::LoopStatusChanged {
            loop_name: "memory_consolidation".to_string(),
            enabled: true,
        };
        let cloned = event.clone();
        if let SystemEvent::LoopStatusChanged { loop_name, enabled } = cloned {
            assert_eq!(loop_name, "memory_consolidation");
            assert!(enabled);
        } else {
            panic!("expected LoopStatusChanged");
        }
    }
}
