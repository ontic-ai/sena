//! Typed prompt segments â€” the only way to supply content to PromptComposer.
//!
//! No static strings are ever passed to the composer. All content comes from
//! typed structs carrying live data from the bus or working memory.

use bus::events::ctp::ContextSnapshot;
use bus::events::memory::MemoryChunk;
use bus::events::soul::{RichSoulSummary, SoulSummary};

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
    /// Rich structured soul summary with sections and relevance scores.
    RichSoulContext(RichSoulSummary),
    /// Relevant long-term memory chunks retrieved from ech0.
    LongTermMemory(Vec<MemoryChunk>),
    /// Current computing context assembled by the CTP subsystem.
    CurrentContext(Box<ContextSnapshot>),
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

            PromptSegment::RichSoulContext(rich_summary) => {
                use bus::events::soul::SoulSectionType;

                if rich_summary.sections.is_empty() {
                    return None;
                }

                // Sort sections by relevance score descending
                let mut sorted_sections = rich_summary.sections.clone();
                sorted_sections.sort_by(|a, b| {
                    b.relevance_score
                        .partial_cmp(&a.relevance_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });

                let formatted_sections: Vec<String> = sorted_sections
                    .iter()
                    .map(|section| {
                        let section_name = match section.section_type {
                            SoulSectionType::RecentEvents => "Recent Activity",
                            SoulSectionType::IdentitySignals => "Identity",
                            SoulSectionType::TemporalHabits => "Temporal Patterns",
                            SoulSectionType::Preferences => "Preferences",
                        };
                        format!("## {}\n{}", section_name, section.content)
                    })
                    .collect();

                Some(formatted_sections.join("\n\n"))
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
                if let Some(task) = &snapshot.inferred_task {
                    parts.push(format!(
                        "Inferred task: {} (confidence: {:.2})",
                        task.semantic_description, task.confidence
                    ));
                }
                if let Some(state) = &snapshot.user_state {
                    parts.push(format!(
                        "User state: frustration={}, flow={}, switch_cost={}",
                        state.frustration_level, state.flow_detected, state.context_switch_cost
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
#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::{ContextSnapshot, EnrichedInferredTask, UserState};
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use bus::events::soul::{RichSoulSummary, SoulSection, SoulSectionType};
    use std::time::{Duration, Instant};

    fn make_snapshot() -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("main.rs".to_string()),
                bundle_id: None,
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 120.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(5),
                timestamp: now,
            },
            session_duration: Duration::from_secs(3600),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: now,
        }
    }

    #[test]
    fn rich_soul_context_segment_assembles_sections() {
        let section1 = SoulSection {
            section_type: SoulSectionType::RecentEvents,
            content: "Recent event content".to_string(),
            relevance_score: 0.9,
        };
        let section2 = SoulSection {
            section_type: SoulSectionType::IdentitySignals,
            content: "Identity content".to_string(),
            relevance_score: 0.7,
        };
        let rich_summary = RichSoulSummary {
            sections: vec![section1, section2],
            token_count: 50,
            request_id: 1,
        };

        let segment = PromptSegment::RichSoulContext(rich_summary);
        let result = segment.to_text().expect("should render");

        // Should be sorted by relevance descending (0.9 before 0.7)
        assert!(result.contains("Recent Activity"));
        assert!(result.contains("Recent event content"));
        assert!(result.contains("Identity"));
        assert!(result.contains("Identity content"));
        // Recent Activity (0.9) should appear before Identity (0.7)
        let recent_pos = result.find("Recent Activity").unwrap();
        let identity_pos = result.find("Identity").unwrap();
        assert!(recent_pos < identity_pos);
    }

    #[test]
    fn rich_soul_context_empty_is_skipped() {
        let rich_summary = RichSoulSummary {
            sections: vec![],
            token_count: 0,
            request_id: 1,
        };
        let segment = PromptSegment::RichSoulContext(rich_summary);
        assert!(segment.to_text().is_none());
    }

    #[test]
    fn current_context_shows_enriched_task() {
        let mut snapshot = make_snapshot();
        snapshot.inferred_task = Some(EnrichedInferredTask {
            category: "coding".to_string(),
            semantic_description: "Editing Rust code in VSCode".to_string(),
            confidence: 0.85,
        });

        let segment = PromptSegment::CurrentContext(Box::new(snapshot));
        let result = segment.to_text().expect("should render");

        assert!(result.contains("Editing Rust code in VSCode"));
        assert!(result.contains("0.85"));
    }

    #[test]
    fn current_context_shows_user_state() {
        let mut snapshot = make_snapshot();
        snapshot.user_state = Some(UserState {
            frustration_level: 45,
            flow_detected: true,
            context_switch_cost: 60,
        });

        let segment = PromptSegment::CurrentContext(Box::new(snapshot));
        let result = segment.to_text().expect("should render");

        assert!(result.contains("frustration=45"));
        assert!(result.contains("flow=true"));
        assert!(result.contains("switch_cost=60"));
    }
}
