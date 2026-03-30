//! Transparency query handler — responds to user queries about observations.
//!
//! Handles TransparencyQuery::CurrentObservation requests by assembling
//! the current context snapshot from the signal buffer.

use std::time::Instant;

use bus::events::transparency::{ObservationResponse, TransparencyQuery};

use crate::context_assembler::ContextAssembler;
use crate::signal_buffer::SignalBuffer;

/// Error type for transparency query handling.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// Query type is not yet supported by this actor.
    #[error("unsupported query type")]
    UnsupportedQuery,
}

/// Handle a transparency query and produce a response.
///
/// For CurrentObservation queries, assembles the current context snapshot
/// from the signal buffer and returns it as an ObservationResponse.
/// Other query types are not yet supported (handled by other actors).
pub fn handle_transparency_query(
    query: TransparencyQuery,
    buffer: &SignalBuffer,
    assembler: &ContextAssembler,
    session_start: Instant,
) -> Result<ObservationResponse, QueryError> {
    match query {
        TransparencyQuery::CurrentObservation => {
            let snapshot = assembler.assemble(buffer, session_start);
            Ok(ObservationResponse { snapshot })
        }
        TransparencyQuery::UserMemory | TransparencyQuery::InferenceExplanation => {
            Err(QueryError::UnsupportedQuery)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_handle_transparency_query_returns_current_observation() {
        let buffer = SignalBuffer::new(Duration::from_secs(300));
        let assembler = ContextAssembler::new();
        let session_start = Instant::now();

        let query = TransparencyQuery::CurrentObservation;
        let result = handle_transparency_query(query, &buffer, &assembler, session_start);

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.snapshot.active_app.app_name, "Unknown");
    }

    #[test]
    fn test_handle_transparency_query_rejects_user_memory() {
        let buffer = SignalBuffer::new(Duration::from_secs(300));
        let assembler = ContextAssembler::new();
        let session_start = Instant::now();

        let query = TransparencyQuery::UserMemory;
        let result = handle_transparency_query(query, &buffer, &assembler, session_start);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), QueryError::UnsupportedQuery));
    }

    #[test]
    fn test_handle_transparency_query_rejects_inference_explanation() {
        let buffer = SignalBuffer::new(Duration::from_secs(300));
        let assembler = ContextAssembler::new();
        let session_start = Instant::now();

        let query = TransparencyQuery::InferenceExplanation;
        let result = handle_transparency_query(query, &buffer, &assembler, session_start);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), QueryError::UnsupportedQuery));
    }
}
