//! Task inference engine — derives an inferred task from signal context.
//!
//! BONES stub: engine skeleton is present but inference is not yet wired to
//! the LLM. When the inference actor is available, this engine will delegate
//! to it via the bus.

use crate::signal_buffer::SignalBuffer;
use bus::events::ctp::EnrichedInferredTask;

/// Derives an inferred task from the current signal buffer state.
///
/// In the full implementation this will send a prompt to the inference actor
/// and await an `InferenceResponse` event. For now it returns a `None` stub.
pub struct TaskInferenceEngine;

impl TaskInferenceEngine {
    /// Create a new task inference engine.
    pub fn new() -> Self {
        Self
    }

    /// Attempt to infer the user's current task from signal history.
    ///
    /// Returns `None` in the BONES stub. Will return a typed task once the
    /// LLM inference path is wired in.
    pub fn infer(&self, _buffer: &SignalBuffer) -> Option<EnrichedInferredTask> {
        // TODO M2: wire to InferenceActor via bus event request/response
        None
    }
}

impl Default for TaskInferenceEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal_buffer::SignalBuffer;
    use std::time::Duration;

    #[test]
    fn infer_returns_none_in_stub() {
        let engine = TaskInferenceEngine::new();
        let buffer = SignalBuffer::new(Duration::from_secs(300));
        assert!(engine.infer(&buffer).is_none());
    }
}
