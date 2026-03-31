//! Transparency query handler — responds to user queries about observations.
//!
//! Handles `TransparencyQuery::CurrentObservation` only. Other query variants
//! (`UserMemory`, `InferenceExplanation`) are handled by the memory and
//! inference actors respectively; CTP silently ignores them.

use bus::events::ctp::ContextSnapshot;
use bus::events::transparency::ObservationResponse;

/// Assemble a `CurrentObservation` response from the current signal buffer.
///
/// This is the only transparency query CTP owns. All other query types
/// must be routed to their respective actors.
pub fn handle_current_observation(snapshot: ContextSnapshot) -> ObservationResponse {
    ObservationResponse { snapshot }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use std::time::{Duration, Instant};

    #[test]
    fn handle_current_observation_returns_snapshot() {
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
            },
            session_duration: Duration::from_secs(5),
            inferred_task: None,
            timestamp: Instant::now(),
        };

        let response = handle_current_observation(snapshot);
        assert_eq!(response.snapshot.active_app.app_name, "Code");
    }
}
