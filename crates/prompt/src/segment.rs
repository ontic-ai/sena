//! Typed prompt segments â€” the only way to supply content to PromptComposer.
//!
//! No static strings are ever passed to the composer. All content comes from
//! typed structs carrying live data from the bus or working memory.

use bus::events::ctp::ContextSnapshot;
use bus::events::memory::MemoryChunk;
use bus::events::soul::SoulSummary;

/// Reflection control mode for prompt-driven reasoning.
pub enum ReflectionMode {
    /// One-pass response generation.
    SingleShot,
    /// Multi-round memory interleave mode.
    Iterative {
        current_round: usize,
        max_rounds: usize,
    },
}

/// A typed segment of prompt content.
///
/// Each variant carries live, typed data â€” never a raw string literal.
pub enum PromptSegment {
    /// Persona and recent event context from the Soul subsystem.
    SoulContext(SoulSummary),
    /// Relevant long-term memory chunks retrieved from ech0.
    LongTermMemory(Vec<MemoryChunk>),
    /// Current computing context assembled by the CTP subsystem.
    CurrentContext(ContextSnapshot),
    /// In-RAM working memory snippets from the current inference cycle.
    WorkingMemorySnippets(Vec<String>),
    /// Reflection control directive for the inference actor.
    ReflectionDirective(ReflectionMode),
}

impl PromptSegment {
    /// Render this segment to a text block, or `None` if the segment is empty.
    pub(crate) fn to_text(&self) -> Option<String> {
        match self {
            PromptSegment::SoulContext(summary) => {
                if summary.content.is_empty() {
                    None
                } else {
                    Some(format!("## Context\n{}", summary.content))
                }
            }

            PromptSegment::LongTermMemory(chunks) => {
                if chunks.is_empty() {
                    return None;
                }
                let lines: Vec<String> = chunks.iter().map(|c| format!("- {}", c.text)).collect();
                Some(format!("## Relevant Memory\n{}", lines.join("\n")))
            }

            PromptSegment::CurrentContext(snapshot) => {
                let mut parts = vec![format!("Active app: {}", snapshot.active_app.app_name)];
                if let Some(title) = &snapshot.active_app.window_title {
                    parts.push(format!("Window: {}", title));
                }
                if !snapshot.recent_files.is_empty() {
                    let files: Vec<String> = snapshot
                        .recent_files
                        .iter()
                        .map(|e| e.path.display().to_string())
                        .collect();
                    parts.push(format!("Recent files: {}", files.join(", ")));
                }
                if let Some(hint) = &snapshot.inferred_task {
                    parts.push(format!(
                        "Inferred task: {} (confidence: {:.2})",
                        hint.category, hint.confidence
                    ));
                }
                Some(format!("## Current Context\n{}", parts.join("\n")))
            }

            PromptSegment::WorkingMemorySnippets(snippets) => {
                if snippets.is_empty() {
                    return None;
                }
                Some(format!("## Working Memory\n{}", snippets.join("\n")))
            }

            PromptSegment::ReflectionDirective(mode) => match mode {
                ReflectionMode::SingleShot => Some("## Reflection\nsingle-shot".to_owned()),
                ReflectionMode::Iterative {
                    current_round,
                    max_rounds,
                } => Some(format!(
                    "## Reflection\niterative {}/{}",
                    current_round, max_rounds
                )),
            },
        }
    }
}
