//! System-level events: boot, shutdown, failure.

/// Information about a failed actor.
#[derive(Debug, Clone)]
pub struct ActorFailureInfo {
    /// Static name of the actor that failed.
    pub actor_name: &'static str,
    /// Error message describing the failure.
    pub error_msg: String,
}

/// System lifecycle and control events.
#[derive(Debug, Clone)]
pub enum SystemEvent {
    /// Signal to initiate graceful shutdown.
    ShutdownSignal,
    /// Boot sequence completed successfully.
    BootComplete,
    /// Emitted when this is the first time Sena has been run.
    FirstBoot,
    /// An actor has failed.
    ActorFailed(ActorFailureInfo),
    /// Emitted by each actor when it has successfully started and is ready.
    ActorReady { actor_name: &'static str },
    /// Encryption subsystem initialized successfully.
    EncryptionInitialized,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_failure_info_constructs() {
        let info = ActorFailureInfo {
            actor_name: "test_actor",
            error_msg: "test error".to_string(),
        };
        assert_eq!(info.actor_name, "test_actor");
        assert_eq!(info.error_msg, "test error");
    }

    #[test]
    fn system_events_are_cloneable() {
        let event = SystemEvent::BootComplete;
        let cloned = event.clone();
        assert!(matches!(cloned, SystemEvent::BootComplete));
    }
}
