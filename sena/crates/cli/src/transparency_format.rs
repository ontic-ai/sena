//! Transparency response formatting for user-facing output.
//!
//! Converts structured transparency responses into rich, human-readable text
//! with semantic formatting, consistent with PR Principle P7 (transparency).

use bus::events::transparency::{MemoryResponse, ObservationResponse, ReasoningResponse};

/// Format CurrentObservation response with structured key/value display.
pub fn format_observation_response(resp: &ObservationResponse) -> String {
    let snapshot = &resp.snapshot;
    let app = &snapshot.active_app.app_name;
    let task = match &snapshot.inferred_task {
        Some(hint) => format!("{} ({:.0}%)", hint.category, hint.confidence * 100.0),
        None => "(no task inferred)".to_string(),
    };
    let clipboard = if snapshot.clipboard_digest.is_some() {
        "clipboard ready"
    } else {
        "no clipboard"
    };
    let rate = snapshot.keystroke_cadence.events_per_minute;
    let secs = snapshot.session_duration.as_secs();
    let session = if secs >= 60 {
        format!("{} min {} sec", secs / 60, secs % 60)
    } else {
        format!("{secs} sec")
    };

    format!(
        "Window:      {app}\n\
         Task:        {task}\n\
         Clipboard:   {clipboard}\n\
         Keyboard:    {rate:.1} events/min\n\
         Session:     {session}"
    )
}

/// Format UserMemory response with soul summary and memory list.
pub fn format_memory_response(resp: &MemoryResponse) -> String {
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

    let mut out = format!(
        "Soul Summary\n\
         Work patterns:  {patterns}\n\
         Tools:          {preferences}\n\
         Interests:      {interests}\n\
         \n\
         Recent Memories"
    );

    if resp.memory_chunks.is_empty() {
        out.push_str("\n  (no retrievable memory snippets are available right now)");
    } else {
        for (i, chunk) in resp.memory_chunks.iter().enumerate() {
            let preview = if chunk.content.chars().count() > 120 {
                let truncated: String = chunk.content.chars().take(120).collect();
                format!("{}...", truncated)
            } else {
                chunk.content.clone()
            };
            out.push_str(&format!(
                "\n  [{}]  {preview}\n       score: {:.2}",
                i + 1,
                chunk.score
            ));
        }
    }

    out
}

/// Format ReasoningChain response with inference details.
pub fn format_reasoning_response(resp: &ReasoningResponse) -> String {
    if resp.causal_id == 0 {
        return format!(
            "Last Inference\n\
             {}\n\
             Try '/explanation latest' if an inference has completed.",
            resp.response_preview
        );
    }

    format!(
        "Last Inference\n\
         Causal ID:  {}\n\
         Source:     {}\n\
         Tokens:     {}\n\
         Preview:    {}",
        resp.causal_id, resp.source_description, resp.token_count, resp.response_preview
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::ctp::ContextSnapshot;
    use bus::events::memory::MemoryChunk;
    use bus::events::platform::{KeystrokeCadence, WindowContext};
    use bus::events::transparency::SoulSummary;
    use std::time::{Duration, Instant};

    #[test]
    fn format_observation_response_renders_snapshot() {
        let snapshot = ContextSnapshot {
            active_app: WindowContext {
                app_name: "Code".to_string(),
                window_title: Some("workspace".to_string()),
                bundle_id: None,
                timestamp: Instant::now(),
            },
            recent_files: Vec::new(),
            clipboard_digest: Some("digest".to_string()),
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 42.5,
                burst_detected: false,
                idle_duration: Duration::from_secs(1),
                timestamp: Instant::now(),
            },
            session_duration: Duration::from_secs(125),
            inferred_task: None,
            user_state: None,
            visual_context: None,
            timestamp: Instant::now(),
            soul_identity_signal: None,
        };

        let response = ObservationResponse {
            snapshot: Box::new(snapshot),
        };

        let formatted = format_observation_response(&response);
        assert!(formatted.contains("Code"));
        assert!(formatted.contains("42.5"));
        assert!(formatted.contains("2 min 5 sec"));
    }

    #[test]
    fn format_memory_response_renders_summary_and_chunks() {
        let response = MemoryResponse {
            soul_summary: SoulSummary {
                inference_cycle_count: 42,
                work_patterns: vec!["morning_coder".to_string()],
                tool_preferences: vec!["vscode".to_string()],
                interest_clusters: vec!["rust".to_string()],
            },
            memory_chunks: vec![MemoryChunk {
                content: "User prefers Rust for systems programming".to_string(),
                score: 0.95,
                age_seconds: 100,
            }],
        };

        let formatted = format_memory_response(&response);
        assert!(formatted.contains("Soul Summary"));
        assert!(formatted.contains("morning_coder"));
        assert!(formatted.contains("vscode"));
        assert!(formatted.contains("rust"));
        assert!(formatted.contains("Recent Memories"));
        assert!(formatted.contains("User prefers Rust"));
        assert!(formatted.contains("0.95"));
    }

    #[test]
    fn format_reasoning_response_renders_inference_details() {
        let response = ReasoningResponse {
            causal_id: 12345,
            source_description: "user voice input".to_string(),
            token_count: 256,
            response_preview: "This is a preview of the response...".to_string(),
        };

        let formatted = format_reasoning_response(&response);
        assert!(formatted.contains("12345"));
        assert!(formatted.contains("user voice input"));
        assert!(formatted.contains("256"));
        assert!(formatted.contains("preview of the response"));
    }

    #[test]
    fn format_reasoning_response_handles_no_inference() {
        let response = ReasoningResponse {
            causal_id: 0,
            source_description: "none".to_string(),
            token_count: 0,
            response_preview: "No inference cycle completed yet".to_string(),
        };

        let formatted = format_reasoning_response(&response);
        assert!(formatted.contains("No inference cycle completed yet"));
        assert!(formatted.contains("/explanation latest"));
    }
}
