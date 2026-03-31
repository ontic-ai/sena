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
    /// Encryption initialization failed.
    EncryptionFailed { reason: String },
    /// OS keychain was unavailable (fell back to passphrase mode).
    KeychainUnavailable,
    /// User clicked a tray menu item.
    TrayMenuClicked(TrayMenuItem),
    /// Tray initialized successfully.
    TrayReady,
    /// Tray initialization failed (non-fatal — Sena continues without tray).
    TrayUnavailable { reason: String },
    /// Memory usage exceeded configured threshold.
    MemoryThresholdExceeded { current_mb: usize, limit_mb: usize },
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

    // Compile-time verification: SystemEvent and ActorFailureInfo are Send
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}

    #[test]
    fn types_are_send() {
        assert_send::<SystemEvent>();
        assert_send::<ActorFailureInfo>();
    }
}
