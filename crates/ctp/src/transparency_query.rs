//! Transparency query handler — responds to user queries about observations.
//!
//! Handles `TransparencyQuery::CurrentObservation` only. Other query variants
//! (`UserMemory`, `InferenceExplanation`) are handled by the memory and
//! inference actors respectively; CTP silently ignores them.

use std::time::Instant;

use bus::events::transparency::ObservationResponse;

use crate::context_assembler::ContextAssembler;
use crate::signal_buffer::SignalBuffer;

/// Assemble a `CurrentObservation` response from the current signal buffer.
///
/// This is the only transparency query CTP owns. All other query types
/// must be routed to their respective actors.
pub fn handle_current_observation(
    buffer: &SignalBuffer,
    assembler: &ContextAssembler,
    session_start: Instant,
) -> ObservationResponse {
    let snapshot = assembler.assemble(buffer, session_start);
    ObservationResponse { snapshot }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn handle_current_observation_returns_snapshot() {
        let buffer = SignalBuffer::new(Duration::from_secs(300));
        let assembler = ContextAssembler::new();
        let session_start = Instant::now();

        let response = handle_current_observation(&buffer, &assembler, session_start);
        assert_eq!(response.snapshot.active_app.app_name, "Unknown");
    }
}
