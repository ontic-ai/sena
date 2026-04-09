//! Rich Soul summary assembly: multi-section structured summaries.

use crate::error::SoulError;
use crate::schema::{EVENT_LOG, IDENTITY_SIGNALS};
use bus::events::soul::{RichSoulSummary, SoulSection, SoulSectionType};
use redb::{ReadableDatabase, ReadableTable};

/// Summary assembler that builds rich, structured Soul summaries.
pub(crate) struct SummaryAssembler;

impl SummaryAssembler {
    /// Build a rich multi-section summary from Soul database state.
    ///
    /// `token_budget` limits total output size (rough estimate: 4 chars ≈ 1 token).
    pub(crate) fn assemble(
        db: &redb::Database,
        token_budget: usize,
    ) -> Result<RichSoulSummary, SoulError> {
        let mut sections = Vec::new();

        // Section 1: Recent events (relevance 0.6)
        if let Ok(recent_events_content) = Self::read_recent_events(db, 50) {
            if !recent_events_content.is_empty() {
                sections.push(SoulSection {
                    section_type: SoulSectionType::RecentEvents,
                    content: recent_events_content,
                    relevance_score: 0.6,
                });
            }
        }

        // Section 2: Identity signals (relevance 0.9)
        if let Ok(identity_signals_content) = Self::read_identity_signals(db) {
            if !identity_signals_content.is_empty() {
                sections.push(SoulSection {
                    section_type: SoulSectionType::IdentitySignals,
                    content: identity_signals_content,
                    relevance_score: 0.9,
                });
            }
        }

        // Section 3: Temporal patterns (relevance 0.7)
        if let Ok(temporal_content) = Self::read_temporal_patterns(db) {
            if !temporal_content.is_empty() {
                sections.push(SoulSection {
                    section_type: SoulSectionType::TemporalHabits,
                    content: temporal_content,
                    relevance_score: 0.7,
                });
            }
        }

        // Section 4: Preferences (relevance 0.8)
        if let Ok(preferences_content) = Self::read_preferences(db) {
            if !preferences_content.is_empty() {
                sections.push(SoulSection {
                    section_type: SoulSectionType::Preferences,
                    content: preferences_content,
                    relevance_score: 0.8,
                });
            }
        }

        // Sort by relevance (highest first)
        sections.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply token budget
        let (final_sections, token_count) = Self::apply_token_budget(sections, token_budget);

        Ok(RichSoulSummary {
            sections: final_sections,
            token_count,
            request_id: 0, // Caller will set this
        })
    }

    fn read_recent_events(db: &redb::Database, max_events: usize) -> Result<String, SoulError> {
        let read_txn = db.begin_read()?;
        let log = read_txn.open_table(EVENT_LOG)?;

        let entries: Vec<String> = log
            .iter()?
            .rev()
            .take(max_events)
            .map(|r| {
                let (_, v) = r?;
                Ok(String::from_utf8_lossy(v.value()).into_owned())
            })
            .collect::<Result<Vec<_>, redb::StorageError>>()?;

        Ok(entries.join("\n"))
    }

    fn read_identity_signals(db: &redb::Database) -> Result<String, SoulError> {
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(IDENTITY_SIGNALS)?;

        let mut signals: Vec<String> = table
            .iter()?
            .filter_map(|entry| {
                let (k, v) = entry.ok()?;
                let key = k.value();
                // Exclude temporal and preference keys (they have their own sections)
                if key.starts_with("temporal::") || key.starts_with("preference::") {
                    return None;
                }
                Some(format!("{}={}", key, v.value()))
            })
            .collect();

        signals.sort();
        Ok(signals.join("\n"))
    }

    fn read_temporal_patterns(db: &redb::Database) -> Result<String, SoulError> {
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(IDENTITY_SIGNALS)?;

        let mut patterns: Vec<String> = table
            .iter()?
            .filter_map(|entry| {
                let (k, v) = entry.ok()?;
                let key = k.value();
                if key.starts_with("temporal::") {
                    Some(format!("{}={}", key, v.value()))
                } else {
                    None
                }
            })
            .collect();

        patterns.sort();
        Ok(patterns.join("\n"))
    }

    fn read_preferences(db: &redb::Database) -> Result<String, SoulError> {
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(IDENTITY_SIGNALS)?;

        let mut prefs: Vec<String> = table
            .iter()?
            .filter_map(|entry| {
                let (k, v) = entry.ok()?;
                let key = k.value();
                if key.starts_with("preference::") {
                    Some(format!("{}={}", key, v.value()))
                } else {
                    None
                }
            })
            .collect();

        prefs.sort();
        Ok(prefs.join("\n"))
    }

    fn apply_token_budget(
        mut sections: Vec<SoulSection>,
        token_budget: usize,
    ) -> (Vec<SoulSection>, usize) {
        let char_budget = token_budget * 4; // Rough estimate: 4 chars ≈ 1 token
        let mut total_chars = 0;
        let mut kept_sections = Vec::new();

        for section in sections.drain(..) {
            let section_len = section.content.len();
            if total_chars + section_len <= char_budget {
                total_chars += section_len;
                kept_sections.push(section);
            } else {
                // Truncate this section to fit remaining budget
                let remaining = char_budget.saturating_sub(total_chars);
                if remaining > 20 {
                    // Only include if we can fit a meaningful chunk
                    let truncate_at = section
                        .content
                        .char_indices()
                        .map(|(i, _)| i)
                        .nth(remaining.saturating_sub(14)) // Reserve space for suffix
                        .unwrap_or(0);

                    let mut truncated_content = section.content;
                    truncated_content.truncate(truncate_at);
                    truncated_content.push_str("...[truncated]");

                    total_chars += truncated_content.len();
                    kept_sections.push(SoulSection {
                        section_type: section.section_type,
                        content: truncated_content,
                        relevance_score: section.relevance_score,
                    });
                }
                break; // Budget exhausted
            }
        }

        let token_count = total_chars / 4;
        (kept_sections, token_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_schema;
    use redb::{ReadableTable, TableDefinition};
    use tempfile::tempdir;

    fn open_test_db() -> (tempfile::TempDir, redb::Database) {
        let dir = tempdir().expect("should create tempdir");
        let db = redb::Database::create(dir.path().join("test.redb")).expect("should create db");
        apply_schema(&db).expect("should apply schema");
        (dir, db)
    }

    #[test]
    fn assemble_empty_db_produces_empty_sections() {
        let (_dir, db) = open_test_db();

        let summary = SummaryAssembler::assemble(&db, 1000).expect("should assemble");
        assert_eq!(summary.sections.len(), 0);
    }

    #[test]
    fn assemble_populated_db_produces_ordered_sections() {
        let (_dir, db) = open_test_db();

        // Write some test data
        {
            let write_txn = db.begin_write().expect("should begin write");
            {
                let mut log = write_txn.open_table(EVENT_LOG).expect("should open log");
                log.insert(1u64, "event 1".as_bytes())
                    .expect("should insert");
                log.insert(2u64, "event 2".as_bytes())
                    .expect("should insert");
            }
            {
                let mut signals = write_txn
                    .open_table(IDENTITY_SIGNALS)
                    .expect("should open signals");
                signals
                    .insert("tool_pref::code", "100")
                    .expect("should insert");
                signals
                    .insert("temporal::hour::14::inference", "50")
                    .expect("should insert");
                signals
                    .insert("preference::verbosity", "low")
                    .expect("should insert");
            }
            write_txn.commit().expect("should commit");
        }

        let summary = SummaryAssembler::assemble(&db, 10000).expect("should assemble");

        // Should have 4 sections
        assert_eq!(summary.sections.len(), 4);

        // Should be sorted by relevance (identity signals = 0.9 should be first)
        assert!(matches!(
            summary.sections[0].section_type,
            SoulSectionType::IdentitySignals
        ));
    }

    #[test]
    fn token_budget_truncates_sections() {
        let (_dir, db) = open_test_db();

        // Write large content
        {
            let write_txn = db.begin_write().expect("should begin write");
            {
                let mut log = write_txn.open_table(EVENT_LOG).expect("should open log");
                for i in 0..100 {
                    let entry = format!("event {}", i);
                    log.insert(i, entry.as_bytes()).expect("should insert");
                }
            }
            write_txn.commit().expect("should commit");
        }

        // Small token budget (100 tokens = ~400 chars)
        let summary = SummaryAssembler::assemble(&db, 100).expect("should assemble");

        // Should have truncated content
        let total_chars: usize = summary.sections.iter().map(|s| s.content.len()).sum();
        assert!(total_chars <= 400);
        assert!(summary.token_count <= 100);
    }

    #[test]
    fn sections_are_sorted_by_relevance() {
        let (_dir, db) = open_test_db();

        {
            let write_txn = db.begin_write().expect("should begin write");
            {
                let mut log = write_txn.open_table(EVENT_LOG).expect("should open log");
                log.insert(1u64, "event".as_bytes()).expect("should insert");
            }
            {
                let mut signals = write_txn
                    .open_table(IDENTITY_SIGNALS)
                    .expect("should open signals");
                signals.insert("key", "value").expect("should insert");
                signals
                    .insert("preference::test", "high")
                    .expect("should insert");
                signals
                    .insert("temporal::hour::12::work", "10")
                    .expect("should insert");
            }
            write_txn.commit().expect("should commit");
        }

        let summary = SummaryAssembler::assemble(&db, 10000).expect("should assemble");

        // Should be sorted: identity (0.9), preferences (0.8), temporal (0.7), recent (0.6)
        assert_eq!(summary.sections.len(), 4);
        assert!(summary.sections[0].relevance_score >= summary.sections[1].relevance_score);
        assert!(summary.sections[1].relevance_score >= summary.sections[2].relevance_score);
        assert!(summary.sections[2].relevance_score >= summary.sections[3].relevance_score);
    }
}
