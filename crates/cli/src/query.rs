//! Query subcommand handler for transparency queries.
//!
//! Implements the `sena query <type>` mode:
//! - Boots runtime
//! - Sends transparency query on bus
//! - Awaits response with 5-second timeout
//! - Formats and displays result

use std::time::Duration;

use anyhow::{anyhow, Result};
use bus::events::transparency::{
    InferenceExplanationResponse, MemoryResponse, ObservationResponse, TransparencyQuery,
};
use bus::{Event, TransparencyEvent};
use tokio::sync::broadcast;

/// Parse query type string to TransparencyQuery enum.
///
/// # Arguments
/// * `query_type` - One of: "observation", "memory", "explanation"
///
/// # Returns
/// TransparencyQuery matching the input type
///
/// # Errors
/// Returns error if query type is not recognized
pub fn parse_query_type(query_type: &str) -> Result<TransparencyQuery> {
    match query_type.to_lowercase().as_str() {
        "observation" => Ok(TransparencyQuery::CurrentObservation),
        "memory" => Ok(TransparencyQuery::UserMemory),
        "explanation" => Ok(TransparencyQuery::InferenceExplanation),
        _ => Err(anyhow!(
            "unknown query type '{}'. Valid types: observation, memory, explanation",
            query_type
        )),
    }
}

/// Execute a transparency query and return formatted output.
///
/// # Implementation
/// 1. Boots runtime (actors are wired inside boot())
/// 2. Creates bus receiver for responses
/// 3. Broadcasts TransparencyQuery on bus
/// 4. Waits for matching response with 5-second timeout
/// 5. Formats response according to query type
/// 6. Gracefully shuts down runtime
///
/// # Arguments
/// * `query` - The TransparencyQuery to send
///
/// # Returns
/// Formatted output string ready to print
///
/// # Errors
/// - Boot failure
/// - Query parsing failure
/// - No matching response received or timeout
pub async fn execute_query(query: TransparencyQuery) -> Result<String> {
    // Boot runtime (actors are wired inside boot())
    let runtime = runtime::boot().await?;

    // Get the bus from runtime (publicly available)
    let bus = &runtime.bus;
    let mut rx = bus.subscribe_broadcast();

    // Send the query on the bus
    bus.broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
        query.clone(),
    )))
    .await?;

    // Await response with 5-second timeout
    let result =
        tokio::time::timeout(Duration::from_secs(5), wait_for_response(query, &mut rx)).await;

    // Shutdown: wait for actors to stop
    let shutdown_timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    runtime::shutdown(runtime, shutdown_timeout).await?;

    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!(
            "query timed out after 5 seconds. No response received."
        )),
    }
}

/// Wait for a response matching the query type.
async fn wait_for_response(
    query: TransparencyQuery,
    rx: &mut broadcast::Receiver<Event>,
) -> Result<String> {
    loop {
        match rx.recv().await {
            Ok(Event::Transparency(event)) => {
                match (query.clone(), event) {
                    (
                        TransparencyQuery::CurrentObservation,
                        TransparencyEvent::ObservationResponded(resp),
                    ) => {
                        return Ok(format_observation_response(&resp));
                    }
                    (TransparencyQuery::UserMemory, TransparencyEvent::MemoryResponded(resp)) => {
                        return Ok(format_memory_response(&resp));
                    }
                    (
                        TransparencyQuery::InferenceExplanation,
                        TransparencyEvent::InferenceExplanationResponded(resp),
                    ) => {
                        return Ok(format_inference_explanation_response(&resp));
                    }
                    _ => {
                        // Not the response we're looking for, keep waiting
                        continue;
                    }
                }
            }
            Ok(_) => {
                // Not a transparency event, keep waiting
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                return Err(anyhow!("bus channel closed before response received"));
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                // Lagged — recovery by resubscribing would lose state; treat as keep-waiting
                continue;
            }
        }
    }
}

/// Format CurrentObservation response for display.
///
/// Output format per unit spec:
/// "Current Context: [app name] | [task] | [clipboard present?] | [keystroke rate] | [session duration]"
fn format_observation_response(resp: &ObservationResponse) -> String {
    let snapshot = &resp.snapshot;
    let app_name = snapshot.active_app.app_name.clone();
    let task = match &snapshot.inferred_task {
        Some(hint) => format!("{} ({:.0}%)", hint.category, hint.confidence * 100.0),
        None => "(no task inferred)".to_string(),
    };
    let clipboard_present = snapshot.clipboard_digest.is_some();
    let keystroke_rate = snapshot.keystroke_cadence.events_per_minute;
    let session_duration_secs = snapshot.session_duration.as_secs();

    format!(
        "Current Context: {} | {} | {} | {:.1} events/min | {} seconds",
        app_name,
        task,
        if clipboard_present {
            "clipboard ready"
        } else {
            "no clipboard"
        },
        keystroke_rate,
        session_duration_secs
    )
}

/// Format UserMemory response for display.
///
/// Output format per unit spec:
/// "Soul Summary: [patterns] [preferences] [interests]\nRecent Memories:\n  [chunk text] (score: X)"
fn format_memory_response(resp: &MemoryResponse) -> String {
    let summary = &resp.soul_summary;

    let patterns = if summary.work_patterns.is_empty() {
        "(none detected)".to_string()
    } else {
        summary.work_patterns.join(", ")
    };

    let preferences = if summary.tool_preferences.is_empty() {
        "(none detected)".to_string()
    } else {
        summary.tool_preferences.join(", ")
    };

    let interests = if summary.interest_clusters.is_empty() {
        "(none detected)".to_string()
    } else {
        summary.interest_clusters.join(", ")
    };

    let mut output = format!(
        "Soul Summary: patterns=[{}] | preferences=[{}] | interests=[{}]",
        patterns, preferences, interests
    );

    output.push_str("\n\nRecent Memories:");
    if resp.memory_chunks.is_empty() {
        output.push_str("\n  (no memories retrieved)");
    } else {
        for chunk in &resp.memory_chunks {
            let text_preview = if chunk.text.len() > 100 {
                format!("{}...", &chunk.text[..100])
            } else {
                chunk.text.clone()
            };
            output.push_str(&format!("\n  {} (score: {:.2})", text_preview, chunk.score));
        }
    }

    output
}

/// Format InferenceExplanation response for display.
///
/// Output format per unit spec:
/// "Last Inference:\nRequest: [context]\nResponse: [first 200 chars]...\nWorking Memory: [N chunks]"
fn format_inference_explanation_response(resp: &InferenceExplanationResponse) -> String {
    let request_preview = if resp.request_context.len() > 200 {
        format!("{}...", &resp.request_context[..200])
    } else {
        resp.request_context.clone()
    };

    let response_preview = if resp.response_text.len() > 200 {
        format!("{}...", &resp.response_text[..200])
    } else {
        resp.response_text.clone()
    };

    format!(
        "Last Inference (Rounds: {}):\n\
         Request: {}\n\
         Response: {}\n\
         Working Memory: {} chunks",
        resp.rounds_completed,
        request_preview,
        response_preview,
        resp.working_memory_context.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_type_observation() {
        let result = parse_query_type("observation").unwrap();
        assert!(matches!(result, TransparencyQuery::CurrentObservation));
    }

    #[test]
    fn parse_query_type_memory() {
        let result = parse_query_type("memory").unwrap();
        assert!(matches!(result, TransparencyQuery::UserMemory));
    }

    #[test]
    fn parse_query_type_explanation() {
        let result = parse_query_type("explanation").unwrap();
        assert!(matches!(result, TransparencyQuery::InferenceExplanation));
    }

    #[test]
    fn parse_query_type_case_insensitive() {
        let result = parse_query_type("OBSERVATION").unwrap();
        assert!(matches!(result, TransparencyQuery::CurrentObservation));

        let result = parse_query_type("Memory").unwrap();
        assert!(matches!(result, TransparencyQuery::UserMemory));
    }

    #[test]
    fn parse_query_type_invalid() {
        let result = parse_query_type("invalid");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unknown query type"));
    }

    #[test]
    fn parse_query_type_empty() {
        let result = parse_query_type("");
        assert!(result.is_err());
    }

    #[test]
    fn format_observation_response_displays_snapshot() {
        let snapshot = bus::events::ctp::ContextSnapshot {
            active_app: bus::events::platform::WindowContext {
                app_name: "VSCode".to_string(),
                window_title: Some("main.rs".to_string()),
                bundle_id: None,
                timestamp: std::time::Instant::now(),
            },
            recent_files: vec![],
            clipboard_digest: Some("abc123def456".to_string()),
            keystroke_cadence: bus::events::platform::KeystrokeCadence {
                events_per_minute: 45.5,
                burst_detected: false,
                idle_duration: Duration::from_secs(10),
            },
            session_duration: Duration::from_secs(3600),
            inferred_task: Some(bus::events::ctp::TaskHint {
                category: "coding".to_string(),
                confidence: 0.92,
            }),
            timestamp: std::time::Instant::now(),
        };

        let resp = ObservationResponse { snapshot };
        let output = format_observation_response(&resp);

        assert!(output.contains("VSCode"));
        assert!(output.contains("coding"));
        assert!(output.contains("clipboard ready"));
        assert!(output.contains("45.5"));
        assert!(output.contains("3600"));
    }

    #[test]
    fn format_observation_response_no_task() {
        let snapshot = bus::events::ctp::ContextSnapshot {
            active_app: bus::events::platform::WindowContext {
                app_name: "Unknown".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: std::time::Instant::now(),
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: bus::events::platform::KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
            },
            session_duration: Duration::from_secs(0),
            inferred_task: None,
            timestamp: std::time::Instant::now(),
        };

        let resp = ObservationResponse { snapshot };
        let output = format_observation_response(&resp);

        assert!(output.contains("no task inferred"));
        assert!(output.contains("no clipboard"));
    }

    #[test]
    fn format_memory_response_with_chunks() {
        let resp = MemoryResponse {
            soul_summary: bus::events::transparency::SoulSummaryForTransparency {
                inference_cycle_count: 10,
                work_patterns: vec!["early_morning".to_string(), "coding_bursts".to_string()],
                tool_preferences: vec!["cargo".to_string(), "vscode".to_string()],
                interest_clusters: vec!["rust".to_string(), "async".to_string()],
            },
            memory_chunks: vec![
                bus::events::memory::MemoryChunk {
                    text: "user prefers async/await patterns".to_string(),
                    score: 0.95,
                    timestamp: std::time::SystemTime::now(),
                },
                bus::events::memory::MemoryChunk {
                    text: "usually codes in the morning".to_string(),
                    score: 0.85,
                    timestamp: std::time::SystemTime::now(),
                },
            ],
        };

        let output = format_memory_response(&resp);

        assert!(output.contains("Soul Summary"));
        assert!(output.contains("early_morning"));
        assert!(output.contains("cargo"));
        assert!(output.contains("rust"));
        assert!(output.contains("Recent Memories"));
        assert!(output.contains("async/await"));
        assert!(output.contains("0.95"));
    }

    #[test]
    fn format_memory_response_no_chunks() {
        let resp = MemoryResponse {
            soul_summary: bus::events::transparency::SoulSummaryForTransparency {
                inference_cycle_count: 0,
                work_patterns: vec![],
                tool_preferences: vec![],
                interest_clusters: vec![],
            },
            memory_chunks: vec![],
        };

        let output = format_memory_response(&resp);

        assert!(output.contains("Soul Summary"));
        assert!(output.contains("(none detected)"));
        assert!(output.contains("(no memories retrieved)"));
    }

    #[test]
    fn format_inference_explanation_response_displays_inference() {
        let resp = InferenceExplanationResponse {
            request_context: "Summarize my work patterns".to_string(),
            response_text: "Based on your keystroke patterns and tool usage, you appear to be a Rust developer...".to_string(),
            working_memory_context: vec![
                bus::events::memory::MemoryChunk {
                    text: "recent pattern analysis".to_string(),
                    score: 0.9,
                    timestamp: std::time::SystemTime::now(),
                },
            ],
            rounds_completed: 2,
        };

        let output = format_inference_explanation_response(&resp);

        assert!(output.contains("Last Inference"));
        assert!(output.contains("Rounds: 2"));
        assert!(output.contains("Summarize my work patterns"));
        assert!(output.contains("Rust developer"));
        assert!(output.contains("1 chunks"));
    }

    #[test]
    fn format_inference_explanation_response_long_text() {
        let long_text = "a".repeat(300);
        let resp = InferenceExplanationResponse {
            request_context: long_text.clone(),
            response_text: long_text.clone(),
            working_memory_context: vec![],
            rounds_completed: 1,
        };

        let output = format_inference_explanation_response(&resp);

        // Should be truncated to 200 chars + "..."
        assert!(output.contains("..."));
        assert!(!output.contains(&long_text)); // Full text should not appear
    }
}
