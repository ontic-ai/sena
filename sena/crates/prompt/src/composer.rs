//! Prompt composer trait and stub implementation.

use crate::{
    error::PromptError,
    segment::PromptSegment,
    types::{ComposedPrompt, PromptContext, PromptTrace},
};
use tracing::{debug, warn};

/// Trait for prompt composition strategies.
///
/// Implementors of this trait define how segments are assembled into a
/// final prompt given a context. Different composers may apply different
/// strategies (e.g., token budgeting, segment ordering, compression).
pub trait PromptComposer: Send + Sync {
    /// Compose a prompt from the given context.
    ///
    /// Returns a `ComposedPrompt` with the assembled text, trace, and token count.
    fn compose(&self, ctx: &PromptContext) -> Result<ComposedPrompt, PromptError>;
}

/// Stub prompt composer for BONES implementation.
///
/// This composer performs minimal segment-based assembly:
/// - Renders each segment with its provenance
/// - Logs each segment using tracing
/// - Returns a placeholder prompt with a full trace
/// - Respects token limits if provided
///
/// Real implementations will perform sophisticated assembly logic,
/// including dynamic segment ordering, token budgeting, and compression.
pub struct StubComposer {
    /// Ordered list of segments to include in the prompt.
    segments: Vec<PromptSegment>,
}

impl StubComposer {
    /// Create a new stub composer with the given segment order.
    pub fn new(segments: Vec<PromptSegment>) -> Self {
        Self { segments }
    }

    /// Create a default stub composer with standard segment ordering.
    pub fn default_segments() -> Self {
        Self::new(vec![
            PromptSegment::SystemInstruction(
                "[BONES:SystemInstruction role=assistant]".to_string(),
            ),
            PromptSegment::SystemPersona,
            PromptSegment::MemoryContext,
            PromptSegment::WorkingMemory,
            PromptSegment::CurrentContext,
            PromptSegment::UserInput,
        ])
    }
}

impl Default for StubComposer {
    fn default() -> Self {
        Self::default_segments()
    }
}

impl PromptComposer for StubComposer {
    fn compose(&self, ctx: &PromptContext) -> Result<ComposedPrompt, PromptError> {
        debug!(
            "StubComposer: starting composition with {} segments",
            self.segments.len()
        );

        let mut trace = PromptTrace::new();
        let mut assembled_parts = Vec::new();
        let mut total_tokens = 0;

        for segment in &self.segments {
            let (text, provenance, token_count) = segment.render(ctx);

            debug!(
                segment = segment.name(),
                provenance = ?provenance,
                token_count = token_count,
                "rendered segment"
            );

            trace.add_segment(segment.name().to_string(), provenance, token_count);
            assembled_parts.push(text);
            total_tokens += token_count;

            // Check token limit if specified
            if let Some(limit) = ctx.token_limit
                && total_tokens > limit
            {
                warn!(
                    current = total_tokens,
                    limit = limit,
                    "token limit exceeded during composition"
                );
                return Err(PromptError::TokenLimitExceeded {
                    current: total_tokens,
                    limit,
                });
            }
        }

        let composed_text = assembled_parts.join("\n\n");

        debug!(
            total_tokens = total_tokens,
            segment_count = trace.segments.len(),
            "composition complete"
        );

        Ok(ComposedPrompt {
            text: composed_text,
            trace,
            token_count: total_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_composer_composes_empty_context() {
        let composer = StubComposer::default_segments();
        let ctx = PromptContext::new();

        let result = composer.compose(&ctx);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.trace.segments.len() > 0);
        assert_eq!(prompt.trace.total_tokens(), prompt.token_count);
    }

    #[test]
    fn stub_composer_includes_user_input() {
        let composer = StubComposer::default_segments();
        let ctx = PromptContext::new().with_user_input("Hello, Sena!".to_string());

        let result = composer.compose(&ctx);
        assert!(result.is_ok());

        let prompt = result.unwrap();
        assert!(prompt.text.contains("Hello, Sena!"));

        // Check that UserInput segment has UserInput provenance
        let user_input_trace = prompt
            .trace
            .segments
            .iter()
            .find(|s| s.segment_name == "UserInput");
        assert!(user_input_trace.is_some());
        assert_eq!(
            user_input_trace.unwrap().provenance,
            crate::types::Provenance::UserInput
        );
    }

    #[test]
    fn stub_composer_respects_token_limit() {
        let composer = StubComposer::default_segments();
        let ctx = PromptContext::new()
            .with_user_input("x".repeat(1000)) // ~250 tokens
            .with_token_limit(10); // Very low limit

        let result = composer.compose(&ctx);
        assert!(result.is_err());

        match result {
            Err(PromptError::TokenLimitExceeded { current, limit }) => {
                assert!(current > limit);
                assert_eq!(limit, 10);
            }
            _ => panic!("expected TokenLimitExceeded error"),
        }
    }

    #[test]
    fn stub_composer_marks_missing_segments() {
        let composer = StubComposer::default_segments();
        let ctx = PromptContext::new(); // No soul summary, memory, etc.

        let result = composer.compose(&ctx);
        assert!(result.is_ok());

        let prompt = result.unwrap();

        // Check that missing segments have Missing provenance
        let persona_trace = prompt
            .trace
            .segments
            .iter()
            .find(|s| s.segment_name == "SystemPersona");
        assert!(persona_trace.is_some());
        assert_eq!(
            persona_trace.unwrap().provenance,
            crate::types::Provenance::Missing
        );
    }

    #[test]
    fn prompt_trace_total_tokens_matches_sum() {
        let mut trace = PromptTrace::new();
        trace.add_segment(
            "Segment1".to_string(),
            crate::types::Provenance::SystemTemplate,
            10,
        );
        trace.add_segment(
            "Segment2".to_string(),
            crate::types::Provenance::UserInput,
            25,
        );

        assert_eq!(trace.total_tokens(), 35);
    }
}
