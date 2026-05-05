//! Transparency query handler — responds to user queries about observations.

use bus::events::ctp::ContextSnapshot;
use bus::events::transparency::ObservationResponse;

/// Assemble a CurrentObservation response from the current context state.
pub fn handle_current_observation(snapshot: ContextSnapshot) -> ObservationResponse {
    ObservationResponse {
        snapshot: Box::new(snapshot),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use std::time::{Duration, Instant};

    #[test]
    fn handle_current_observation_returns_wrapped_snapshot() {
        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "Code".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: Vec::new(),
            clipboard_digest: Some("digest".to_string()),
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 42.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(1),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(5),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: None,
        };

        let response = handle_current_observation(snapshot.clone());
        assert_eq!(
            response.snapshot.active_app.app_name,
            snapshot.active_app.app_name
        );
        assert_eq!(
            response.snapshot.clipboard_digest,
            snapshot.clipboard_digest
        );
    }
}
