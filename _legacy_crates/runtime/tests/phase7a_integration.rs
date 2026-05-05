use std::time::SystemTime;

use bus::events::ctp::{EnrichedInferredTask, SignalPattern, SignalPatternType, UserState};
use bus::events::memory::{ContextMemoryQueryRequest, ContextMemoryQueryResponse, MemoryChunk};
use bus::events::soul::{
    DistilledIdentitySignal, EngagementSignal, RichSoulSummary, SoulSection, SoulSectionType,
    TemporalBehaviorPattern,
};
use prompt::{PromptComposer, PromptSegment};

fn assert_send_sync_static<T: Send + Sync + 'static>() {}

#[test]
fn test_new_bus_events_are_send_and_sync() {
    assert_send_sync_static::<EnrichedInferredTask>();
    assert_send_sync_static::<UserState>();
    assert_send_sync_static::<SignalPattern>();
    assert_send_sync_static::<DistilledIdentitySignal>();
    assert_send_sync_static::<TemporalBehaviorPattern>();
    assert_send_sync_static::<EngagementSignal>();
    assert_send_sync_static::<RichSoulSummary>();
    assert_send_sync_static::<ContextMemoryQueryRequest>();
    assert_send_sync_static::<ContextMemoryQueryResponse>();

    let pattern = SignalPattern {
        pattern_type: SignalPatternType::Frustration,
        confidence: 0.8,
        description: "compile-time coverage".to_string(),
    };
    assert!(matches!(
        pattern.pattern_type,
        SignalPatternType::Frustration
    ));
}

#[test]
fn test_prompt_assembles_with_rich_soul_context() {
    let composer = PromptComposer::new();
    let rich_summary = RichSoulSummary {
        sections: vec![
            SoulSection {
                section_type: SoulSectionType::RecentEvents,
                content: "User returned to the Rust workspace after lunch.".to_string(),
                relevance_score: 0.55,
            },
            SoulSection {
                section_type: SoulSectionType::Preferences,
                content: "preference::verbosity=low".to_string(),
                relevance_score: 0.80,
            },
            SoulSection {
                section_type: SoulSectionType::IdentitySignals,
                content: "frequent_app=vscode".to_string(),
                relevance_score: 0.95,
            },
        ],
        token_count: 48,
        request_id: 41,
    };

    let prompt = composer
        .assemble(&[PromptSegment::RichSoulContext(rich_summary)])
        .expect("rich soul context should assemble into prompt output");

    assert!(prompt.contains("## Identity"));
    assert!(prompt.contains("frequent_app=vscode"));
    assert!(prompt.contains("## Preferences"));
    assert!(prompt.contains("preference::verbosity=low"));
    assert!(prompt.contains("## Recent Activity"));
    assert!(prompt.contains("User returned to the Rust workspace after lunch."));
    assert!(
        prompt
            .find("## Identity")
            .expect("identity header should be present")
            < prompt
                .find("## Preferences")
                .expect("preferences header should be present")
    );
}

#[test]
fn test_context_memory_query_types_round_trip() {
    let request = ContextMemoryQueryRequest {
        context_description: "editing Rust code in main.rs".to_string(),
        max_chunks: 3,
        request_id: 77,
    };
    let response = ContextMemoryQueryResponse {
        chunks: vec![MemoryChunk {
            text: "Sensitive recalled memory about a debugging session".to_string(),
            score: 0.91,
            timestamp: SystemTime::now(),
        }],
        relevance_score: 0.88,
        request_id: request.request_id,
    };

    assert_eq!(request.context_description, "editing Rust code in main.rs");
    assert_eq!(request.max_chunks, 3);
    assert_eq!(response.request_id, 77);
    assert_eq!(response.chunks.len(), 1);
    assert_eq!(response.relevance_score, 0.88);

    let request_debug = format!("{request:?}");
    let response_debug = format!("{response:?}");

    assert!(request_debug.contains("editing Rust code in main.rs"));
    assert!(response_debug.contains("REDACTED"));
    assert!(!response_debug.contains("Sensitive recalled memory"));
}
