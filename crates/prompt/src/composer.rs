//! Prompt assembly from typed segments.
//!
//! PromptComposer is stateless and cheap to construct. Callers create one
//! per inference cycle, assemble segments, and discard it.

use crate::error::PromptError;
use crate::segment::PromptSegment;

/// Assembles typed prompt segments into a final prompt string.
///
/// No static strings are ever injected. All content comes from live, typed
/// data carried by [`PromptSegment`] variants.
pub struct PromptComposer;

impl PromptComposer {
    pub fn new() -> Self {
        Self
    }

    /// Assemble a list of segments into a single prompt string.
    ///
    /// Empty segments (empty lists, empty strings) are silently skipped.
    /// Non-empty segments are joined with `\n\n`.
    ///
    /// Returns [`PromptError::NoSegments`] if no segment produces any text.
    pub fn assemble(&self, segments: &[PromptSegment]) -> Result<String, PromptError> {
        let parts: Vec<String> = segments.iter().filter_map(|s| s.to_text()).collect();

        if parts.is_empty() {
            return Err(PromptError::NoSegments);
        }

        Ok(parts.join("\n\n"))
    }

    /// Assemble segments within a word-count budget.
    ///
    /// Segments are processed in priority order (highest-value first):
    /// `SoulContext` > `CurrentContext` > `LongTermMemory` > `WorkingMemorySnippets` > `ReflectionDirective`
    ///
    /// Lower-priority segments are dropped when adding them would exceed `max_words`.
    /// Word count is approximated via `split_whitespace().count()`.
    ///
    /// Returns [`PromptError::NoSegments`] if no segment fits within the budget.
    pub fn assemble_with_budget(
        &self,
        segments: &[PromptSegment],
        max_words: usize,
    ) -> Result<String, PromptError> {
        // Priority function — lower number = included first.
        fn priority(seg: &PromptSegment) -> u8 {
            match seg {
                PromptSegment::SoulContext(_) => 0,
                PromptSegment::CurrentContext(_) => 1,
                PromptSegment::LongTermMemory(_) => 2,
                PromptSegment::WorkingMemorySnippets(_) => 3,
                PromptSegment::ReflectionDirective(_) => 4,
            }
        }

        // Collect (priority, text) pairs, skipping empty segments.
        let mut rendered: Vec<(u8, String)> = segments
            .iter()
            .filter_map(|s| s.to_text().map(|t| (priority(s), t)))
            .collect();

        // Sort ascending by priority so highest-value segments are considered first.
        rendered.sort_by_key(|(p, _)| *p);

        let mut parts: Vec<String> = Vec::new();
        let mut word_budget = max_words;

        for (_, text) in rendered {
            let word_count = text.split_whitespace().count();
            if word_count <= word_budget {
                word_budget = word_budget.saturating_sub(word_count);
                parts.push(text);
            }
            // Segment doesn't fit — skip it; lower-priority segments may still fit.
        }

        if parts.is_empty() {
            return Err(PromptError::NoSegments);
        }

        Ok(parts.join("\n\n"))
    }
}

impl Default for PromptComposer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::PromptSegment;
    use bus::events::ctp::ContextSnapshot;
    use bus::events::memory::MemoryChunk;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use bus::events::soul::SoulSummary;
    use std::time::{Duration, Instant, SystemTime};

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
    fn empty_segments_returns_no_segments_error() {
        let composer = PromptComposer::new();
        let result = composer.assemble(&[]);
        assert!(matches!(result, Err(PromptError::NoSegments)));
    }

    #[test]
    fn soul_context_assembles_with_content() {
        let composer = PromptComposer::new();
        let summary = SoulSummary {
            content: "persona content".into(),
            event_count: 1,
            request_id: 1,
        };
        let result = composer
            .assemble(&[PromptSegment::SoulContext(summary)])
            .unwrap();
        assert!(result.contains("persona content"));
    }

    #[test]
    fn empty_soul_context_is_skipped() {
        let composer = PromptComposer::new();
        let empty_summary = SoulSummary {
            content: String::new(),
            event_count: 0,
            request_id: 0,
        };
        let result = composer.assemble(&[PromptSegment::SoulContext(empty_summary)]);
        assert!(matches!(result, Err(PromptError::NoSegments)));
    }

    #[test]
    fn memory_chunks_segment_assembles() {
        let composer = PromptComposer::new();
        let chunk = MemoryChunk {
            text: "relevant memory".into(),
            score: 0.9,
            timestamp: SystemTime::now(),
        };
        let result = composer
            .assemble(&[PromptSegment::LongTermMemory(vec![chunk])])
            .unwrap();
        assert!(result.contains("relevant memory"));
    }

    #[test]
    fn empty_memory_chunk_list_is_skipped() {
        let composer = PromptComposer::new();
        let summary = SoulSummary {
            content: "soul ctx".into(),
            event_count: 0,
            request_id: 0,
        };
        let result = composer
            .assemble(&[
                PromptSegment::SoulContext(summary),
                PromptSegment::LongTermMemory(vec![]),
            ])
            .unwrap();
        assert!(result.contains("soul ctx"));
        assert!(!result.contains("Relevant Memory"));
    }

    #[test]
    fn current_context_segment_assembles() {
        let composer = PromptComposer::new();
        let result = composer
            .assemble(&[PromptSegment::CurrentContext(Box::new(make_snapshot()))])
            .unwrap();
        assert!(result.contains("Code"));
        assert!(result.contains("main.rs"));
    }

    #[test]
    fn working_memory_snippets_assembles() {
        let composer = PromptComposer::new();
        let result = composer
            .assemble(&[PromptSegment::WorkingMemorySnippets(vec![
                "snippet1".into(),
                "snippet2".into(),
            ])])
            .unwrap();
        assert!(result.contains("snippet1") && result.contains("snippet2"));
    }

    #[test]
    fn empty_working_memory_is_skipped() {
        let composer = PromptComposer::new();
        let result = composer.assemble(&[PromptSegment::WorkingMemorySnippets(vec![])]);
        assert!(matches!(result, Err(PromptError::NoSegments)));
    }

    #[test]
    fn multiple_segments_joined_with_double_newline() {
        let composer = PromptComposer::new();
        let summary = SoulSummary {
            content: "soul".into(),
            event_count: 0,
            request_id: 0,
        };
        let result = composer
            .assemble(&[
                PromptSegment::SoulContext(summary),
                PromptSegment::WorkingMemorySnippets(vec!["work".into()]),
            ])
            .unwrap();
        assert!(result.contains("\n\n"));
        assert!(result.contains("soul") && result.contains("work"));
    }
}
