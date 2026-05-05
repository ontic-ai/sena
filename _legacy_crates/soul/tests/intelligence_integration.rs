use std::time::{Duration, UNIX_EPOCH};

use bus::events::soul::{EngagementSignal, SoulSectionType};
use soul::schema::{EVENT_LOG, IDENTITY_SIGNALS};
use soul::{apply_schema, DistillationEngine, PreferenceLearner, SummaryAssembler, TemporalModel};
use tempfile::tempdir;

#[test]
fn test_distillation_engine_produces_identity_signals() {
    let mut engine = DistillationEngine::new();

    for count in 1..=25 {
        engine.observe_identity_signal("tool_pref::vscode", &count.to_string());
    }

    let signals = engine.harvest();
    let signal = signals
        .iter()
        .find(|candidate| candidate.signal_key == "frequent_app")
        .expect("distillation should emit a frequent_app signal after enough tool observations");

    assert_eq!(signal.signal_value, "vscode");
    assert!(signal.confidence > 0.0);
    assert!(signal.source_event_count >= 20);
}

#[test]
fn test_temporal_model_records_and_retrieves_patterns() {
    let mut model = TemporalModel::new();

    for minute in 0..5 {
        model.record_event(
            UNIX_EPOCH + Duration::from_secs((9 * 3600) + minute * 60),
            "coding",
        );
    }
    for minute in 0..3 {
        model.record_event(
            UNIX_EPOCH + Duration::from_secs(86_400 + (14 * 3600) + minute * 60),
            "meeting",
        );
    }
    for minute in 0..2 {
        model.record_event(
            UNIX_EPOCH + Duration::from_secs((2 * 86_400) + (20 * 3600) + minute * 60),
            "review",
        );
    }

    let patterns = model.top_patterns(3);

    assert_eq!(patterns.len(), 3);
    assert_eq!(patterns[0].frequency, 5);
    assert_eq!(patterns[1].frequency, 3);
    assert_eq!(patterns[2].frequency, 2);
    assert!(patterns.iter().any(|pattern| {
        pattern.hour_of_day == Some(9)
            && pattern.day_of_week == Some(3)
            && pattern.behavior_category == "coding"
    }));
    assert!(patterns.iter().any(|pattern| {
        pattern.hour_of_day == Some(14)
            && pattern.day_of_week == Some(4)
            && pattern.behavior_category == "meeting"
    }));
}

#[test]
fn test_preference_learner_distills_from_engagements() {
    let mut learner = PreferenceLearner::new();

    for _ in 0..25 {
        learner.record_engagement(&EngagementSignal::Interrupted);
    }

    let preferences = learner.harvest_preferences();

    assert!(preferences
        .iter()
        .any(|(key, value)| { key == "preference::verbosity" && value == "low" }));
}

#[test]
fn test_summary_assembler_produces_rich_structured_output() {
    let temp_dir = tempdir().expect("should create tempdir for soul summary assembly");
    let db_path = temp_dir.path().join("summary.redb");
    let db = redb::Database::create(&db_path).expect("should create temporary soul database");
    apply_schema(&db).expect("should apply soul schema to temporary database");

    {
        let write_txn = db
            .begin_write()
            .expect("should begin soul write transaction");
        {
            let mut log = write_txn
                .open_table(EVENT_LOG)
                .expect("should open event log table");
            log.insert(1_u64, "user opened VS Code".as_bytes())
                .expect("should insert first event");
            log.insert(2_u64, "user resumed coding after a break".as_bytes())
                .expect("should insert second event");
        }
        {
            let mut signals = write_txn
                .open_table(IDENTITY_SIGNALS)
                .expect("should open identity signals table");
            signals
                .insert("frequent_app", "vscode")
                .expect("should insert identity signal");
            signals
                .insert("temporal::hour::9::coding", "7")
                .expect("should insert temporal pattern");
            signals
                .insert("preference::verbosity", "low")
                .expect("should insert preference");
        }
        write_txn
            .commit()
            .expect("should commit temporary soul summary data");
    }

    let summary = SummaryAssembler::assemble(&db, 500)
        .expect("should assemble a rich soul summary from temporary database state");
    assert_eq!(summary.sections.len(), 4);
    assert!(matches!(
        summary.sections[0].section_type,
        SoulSectionType::IdentitySignals
    ));
    assert!(matches!(
        summary.sections[1].section_type,
        SoulSectionType::Preferences
    ));
    assert!(matches!(
        summary.sections[2].section_type,
        SoulSectionType::TemporalHabits
    ));
    assert!(matches!(
        summary.sections[3].section_type,
        SoulSectionType::RecentEvents
    ));
    assert!(summary.sections[0].content.contains("frequent_app=vscode"));
    assert!(summary.sections[1]
        .content
        .contains("preference::verbosity=low"));
    assert!(summary.sections[2]
        .content
        .contains("temporal::hour::9::coding=7"));
    assert!(summary.sections[3]
        .content
        .contains("user resumed coding after a break"));
}
