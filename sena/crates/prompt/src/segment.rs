//! Prompt segment types.

use crate::types::{PromptContext, Provenance};

/// A typed segment of a prompt.
///
/// Each variant represents a specific kind of content that can be included
/// in a composed prompt. Segments are assembled by the composer based on
/// the provided context.
#[derive(Debug, Clone)]
pub enum PromptSegment {
    /// System persona from Soul.
    ///
    /// Contains identity signals, temporal patterns, and learned preferences
    /// that define Sena's personality and interaction style.
    SystemPersona,

    /// Long-term memory context.
    ///
    /// Relevant chunks retrieved from the semantic memory store based on
    /// the current context or user query.
    MemoryContext,

    /// Working memory (ephemeral, in-RAM only).
    ///
    /// Recent conversational context or task state that has not been
    /// consolidated to long-term memory.
    WorkingMemory,

    /// Current context snapshot from CTP.
    ///
    /// Platform signals, active window, temporal state, and other
    /// environmental context from the CTP subsystem.
    CurrentContext,

    /// User input or query.
    ///
    /// The direct input from the user that triggered this composition.
    UserInput,

    /// System instruction or template.
    ///
    /// Static or semi-static instruction text that guides the model's
    /// behavior. This is the only segment type that may contain predefined
    /// text, and it must be minimal and generic.
    SystemInstruction(String),
}

impl PromptSegment {
    /// Render this segment to text given the provided context.
    ///
    /// Returns (rendered_text, provenance, estimated_token_count).
    ///
    /// In this stub implementation, most segments return placeholder text.
    /// Real implementations will extract content from the context.
    pub fn render(&self, ctx: &PromptContext) -> (String, Provenance, usize) {
        match self {
            PromptSegment::SystemPersona => {
                if let Some(ref summary) = ctx.soul_summary {
                    let text = format!(
                        "[BONES:SystemPersona events={} content_len={}]",
                        summary.event_count,
                        summary.content.len()
                    );
                    let tokens = estimate_tokens(&text);
                    (text, Provenance::Soul, tokens)
                } else {
                    (
                        "[BONES:SystemPersona source=missing]".to_string(),
                        Provenance::Missing,
                        0,
                    )
                }
            }
            PromptSegment::MemoryContext => {
                if ctx.memory_chunks.is_empty() {
                    (
                        "[BONES:MemoryContext source=missing]".to_string(),
                        Provenance::Missing,
                        0,
                    )
                } else {
                    let text = format!("[BONES:MemoryContext chunks={}]", ctx.memory_chunks.len());
                    let tokens = estimate_tokens(&text);
                    (text, Provenance::LongTermMemory, tokens)
                }
            }
            PromptSegment::WorkingMemory => {
                if ctx.working_memory.is_empty() {
                    (
                        "[BONES:WorkingMemory source=missing]".to_string(),
                        Provenance::Missing,
                        0,
                    )
                } else {
                    let text =
                        format!("[BONES:WorkingMemory entries={}]", ctx.working_memory.len());
                    let tokens = estimate_tokens(&text);
                    (text, Provenance::WorkingMemory, tokens)
                }
            }
            PromptSegment::CurrentContext => {
                if let Some(ref snapshot) = ctx.snapshot {
                    let text = format!(
                        "[BONES:CurrentContext app={} files={} session_secs={}]",
                        snapshot.active_app.app_name,
                        snapshot.recent_files.len(),
                        snapshot.session_duration.as_secs()
                    );
                    let tokens = estimate_tokens(&text);
                    (text, Provenance::ContextSnapshot, tokens)
                } else {
                    (
                        "[BONES:CurrentContext source=missing]".to_string(),
                        Provenance::Missing,
                        0,
                    )
                }
            }
            PromptSegment::UserInput => {
                if let Some(ref input) = ctx.user_input {
                    let tokens = estimate_tokens(input);
                    (input.clone(), Provenance::UserInput, tokens)
                } else {
                    (
                        "[BONES:UserInput source=missing]".to_string(),
                        Provenance::Missing,
                        0,
                    )
                }
            }
            PromptSegment::SystemInstruction(ref text) => {
                let tokens = estimate_tokens(text);
                (text.clone(), Provenance::SystemTemplate, tokens)
            }
        }
    }

    /// Get the canonical name of this segment for tracing.
    pub fn name(&self) -> &'static str {
        match self {
            PromptSegment::SystemPersona => "SystemPersona",
            PromptSegment::MemoryContext => "MemoryContext",
            PromptSegment::WorkingMemory => "WorkingMemory",
            PromptSegment::CurrentContext => "CurrentContext",
            PromptSegment::UserInput => "UserInput",
            PromptSegment::SystemInstruction(_) => "SystemInstruction",
        }
    }
}

/// Estimate token count for a text string.
///
/// This is a rough heuristic: 1 token ≈ 4 characters.
/// Real implementations should use the tokenizer from the active model.
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}
