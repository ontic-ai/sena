//! CTP (Continuous Thought Processing) events and context types.

use std::time::{Duration, Instant};

use super::platform::{ClipboardDigest, FileEvent, KeystrokeCadence, WindowContext};

/// Structured capture of user's computing context at a moment in time.
#[derive(Debug, Clone)]
pub struct ContextSnapshot {
    /// Currently active application context.
    pub active_app: WindowContext,
    /// Recent file system events.
    pub recent_files: Vec<FileEvent>,
    /// Digest/summary of clipboard content — NOT raw text.
    pub clipboard_digest: Option<ClipboardDigest>,
    /// Keystroke cadence pattern.
    pub keystroke_cadence: KeystrokeCadence,
    /// Session duration since boot.
    pub session_duration: Duration,
    /// When this snapshot was captured.
    pub timestamp: Instant,
}

/// CTP (Continuous Thought Processing) events.
#[derive(Debug, Clone)]
pub enum CTPEvent {
    /// A thought event was triggered by CTP.
    ThoughtEventTriggered(ContextSnapshot),

    /// Signal received and buffered by CTP.
    SignalReceived {
        signal_type: String,
        timestamp: Instant,
    },

    /// CTP loop started successfully.
    LoopStarted,

    /// CTP loop stopped.
    LoopStopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_snapshot_constructs() {
        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "TestApp".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(10),
            timestamp: Instant::now(),
        };
        assert_eq!(snapshot.active_app.app_name, "TestApp");
    }
}
