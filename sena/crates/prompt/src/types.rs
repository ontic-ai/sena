//! Core types for prompt composition.

use serde::{Deserialize, Serialize};

/// Context provided to the composer for prompt assembly.
///
/// This struct aggregates all available context sources that the composer
/// may draw from when assembling a prompt. Not all fields are required for
/// every composition operation.
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// User input text, if any.
    pub user_input: Option<String>,

    /// Current context snapshot from CTP.
    pub snapshot: Option<bus::ContextSnapshot>,

    /// Long-term memory chunks retrieved for this context.
    pub memory_chunks: Vec<memory::ScoredChunk>,

    /// Working memory (ephemeral, not persisted).
    pub working_memory: Vec<String>,

    /// Soul summary for persona/identity context.
    pub soul_summary: Option<soul::SoulSummary>,

    /// Maximum token budget for the composed prompt.
    pub token_limit: Option<usize>,
}

impl PromptContext {
    /// Create an empty context.
    pub fn new() -> Self {
        Self {
            user_input: None,
            snapshot: None,
            memory_chunks: Vec::new(),
            working_memory: Vec::new(),
            soul_summary: None,
            token_limit: None,
        }
    }

    /// Set user input.
    pub fn with_user_input(mut self, input: String) -> Self {
        self.user_input = Some(input);
        self
    }

    /// Set CTP snapshot.
    pub fn with_snapshot(mut self, snapshot: bus::ContextSnapshot) -> Self {
        self.snapshot = Some(snapshot);
        self
    }

    /// Set memory chunks.
    pub fn with_memory_chunks(mut self, chunks: Vec<memory::ScoredChunk>) -> Self {
        self.memory_chunks = chunks;
        self
    }

    /// Set working memory.
    pub fn with_working_memory(mut self, working: Vec<String>) -> Self {
        self.working_memory = working;
        self
    }

    /// Set soul summary.
    pub fn with_soul_summary(mut self, summary: soul::SoulSummary) -> Self {
        self.soul_summary = Some(summary);
        self
    }

    /// Set token limit.
    pub fn with_token_limit(mut self, limit: usize) -> Self {
        self.token_limit = Some(limit);
        self
    }
}

impl Default for PromptContext {
    fn default() -> Self {
        Self::new()
    }
}

/// The final composed prompt ready for inference.
#[derive(Debug, Clone)]
pub struct ComposedPrompt {
    /// The assembled prompt text.
    pub text: String,

    /// Trace showing how the prompt was composed.
    pub trace: PromptTrace,

    /// Total estimated token count.
    pub token_count: usize,
}

/// Provenance tracking for each prompt segment.
///
/// Records where a segment's content came from, enabling transparency
/// and debugging of prompt composition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Provenance {
    /// Segment content came from Soul (persona, identity).
    Soul,

    /// Segment content came from long-term memory.
    LongTermMemory,

    /// Segment content came from working memory (ephemeral).
    WorkingMemory,

    /// Segment content came from CTP context snapshot.
    ContextSnapshot,

    /// Segment content came from user input.
    UserInput,

    /// Segment content is a static system template.
    SystemTemplate,

    /// Segment content is missing (not provided in context).
    Missing,

    /// Segment content came from an external source (e.g., tool output).
    External(String),
}

/// Trace of how a prompt was composed.
///
/// Each entry corresponds to one segment in the final prompt, with
/// provenance and token count recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTrace {
    /// Segment traces in order of composition.
    pub segments: Vec<SegmentTrace>,
}

impl PromptTrace {
    /// Create an empty trace.
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// Add a segment trace.
    pub fn add_segment(
        &mut self,
        segment_name: String,
        provenance: Provenance,
        token_count: usize,
    ) {
        self.segments.push(SegmentTrace {
            segment_name,
            provenance,
            token_count,
        });
    }

    /// Total token count across all segments.
    pub fn total_tokens(&self) -> usize {
        self.segments.iter().map(|s| s.token_count).sum()
    }
}

impl Default for PromptTrace {
    fn default() -> Self {
        Self::new()
    }
}

/// Trace entry for a single prompt segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentTrace {
    /// Name of the segment (e.g., "SystemPersona", "UserInput").
    pub segment_name: String,

    /// Where the segment's content came from.
    pub provenance: Provenance,

    /// Estimated token count for this segment.
    pub token_count: usize,
}
