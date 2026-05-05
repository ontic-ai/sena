//! Identity signal extraction from inference responses.
//!
//! Lightweight keyword-based pattern matching to detect identity-relevant signals
//! from inference exchanges. Extracted signals are emitted to the Soul actor for
//! persistent identity state building.
//!
//! This is NOT inference-based extraction (that would be circular) — just simple
//! keyword/phrase matching on response text.

use std::collections::HashSet;

/// Extract identity signals from an inference response.
///
/// Detects patterns in the response text that indicate behavioral identity signals:
/// - Work domain signals: coding, development, software engineering patterns
/// - Tool preferences: mentions of specific programming tools, editors, languages
/// - Interest clusters: topics discussed in responses
/// - Temporal habits: time-of-day patterns (tracked at call site, not here)
///
/// Returns a Vec of `(key, value)` pairs to be emitted as `IdentitySignalEmitted` events.
pub fn extract_identity_signals(response: &str) -> Vec<(String, String)> {
    let mut signals = Vec::new();
    let lower = response.to_lowercase();

    // Work domain patterns
    if contains_any(&lower, &[
        "code", "coding", "programming", "software", "development", "developer",
        "function", "class", "variable", "algorithm", "debug", "compile",
        "git", "version control", "repository", "commit", "branch",
        "rust", "python", "javascript", "typescript", "api", "fastapi",
    ]) {
        signals.push(("work_domain".to_string(), "software_development".to_string()));
    }

    // Rust programming preference
    if contains_any(&lower, &[
        "rust", "cargo", "rustc", "tokio", "async", "trait", "borrow checker",
    ]) {
        signals.push(("tool_preference".to_string(), "rust".to_string()));
    }

    // Python programming preference
    if contains_any(&lower, &[
        "python", "pip", "numpy", "pandas", "django", "pytorch",
    ]) {
        signals.push(("tool_preference".to_string(), "python".to_string()));
    }

    // JavaScript/TypeScript preference
    if contains_any(&lower, &[
        "javascript", "typescript", "node", "npm", "react", "vue", "angular",
    ]) {
        signals.push(("tool_preference".to_string(), "javascript".to_string()));
    }

    // AI/ML interest cluster
    if contains_any(&lower, &[
        "machine learning", "neural network", "deep learning", "model training",
        "inference", "transformer", "embeddings", "llm", "language model",
    ]) {
        signals.push(("interest".to_string(), "artificial_intelligence".to_string()));
    }

    // Systems programming interest
    if contains_any(&lower, &[
        "memory management", "concurrency", "threading", "async runtime",
        "low-level", "performance optimization", "systems programming",
    ]) {
        signals.push(("interest".to_string(), "systems_programming".to_string()));
    }

    // Web development interest
    if contains_any(&lower, &[
        "web development", "frontend", "backend", "api", "rest", "graphql",
        "http", "server", "client", "browser",
    ]) {
        signals.push(("interest".to_string(), "web_development".to_string()));
    }

    // Data science interest
    if contains_any(&lower, &[
        "data science", "data analysis", "statistics", "visualization",
        "dataset", "data processing", "analytics",
    ]) {
        signals.push(("interest".to_string(), "data_science".to_string()));
    }

    // Deduplicate signals (multiple mentions in one response → one signal)
    deduplicate_signals(signals)
}

/// Check if the text contains any of the given patterns.
fn contains_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| text.contains(p))
}

/// Deduplicate signals by keeping only the first occurrence of each (key, value) pair.
fn deduplicate_signals(signals: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    signals
        .into_iter()
        .filter(|sig| seen.insert(sig.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_software_development_signal() {
        let response = "To debug this code, you should add logging statements before the function call.";
        let signals = extract_identity_signals(response);
        assert!(signals.contains(&("work_domain".to_string(), "software_development".to_string())));
    }

    #[test]
    fn test_extract_rust_tool_preference() {
        let response = "You can use tokio for async runtime in Rust. The borrow checker will ensure memory safety.";
        let signals = extract_identity_signals(response);
        assert!(signals.contains(&("work_domain".to_string(), "software_development".to_string())));
        assert!(signals.contains(&("tool_preference".to_string(), "rust".to_string())));
        assert!(signals.contains(&("interest".to_string(), "systems_programming".to_string())));
    }

    #[test]
    fn test_extract_python_tool_preference() {
        let response = "Use pandas to load the dataset and numpy for numerical operations.";
        let signals = extract_identity_signals(response);
        assert!(signals.contains(&("tool_preference".to_string(), "python".to_string())));
        assert!(signals.contains(&("interest".to_string(), "data_science".to_string())));
    }

    #[test]
    fn test_extract_ai_ml_interest() {
        let response = "The transformer model uses attention mechanisms for language understanding.";
        let signals = extract_identity_signals(response);
        assert!(signals.contains(&("interest".to_string(), "artificial_intelligence".to_string())));
    }

    #[test]
    fn test_extract_web_development_interest() {
        let response = "The REST API endpoint should return JSON with proper HTTP status codes.";
        let signals = extract_identity_signals(response);
        assert!(signals.contains(&("interest".to_string(), "web_development".to_string())));
    }

    #[test]
    fn test_deduplication() {
        let response = "Rust is great. I love Rust. Rust has a borrow checker. Use Rust for systems programming.";
        let signals = extract_identity_signals(response);
        // Should only have one rust signal despite multiple mentions
        let rust_signals: Vec<_> = signals
            .iter()
            .filter(|(k, v)| k == "tool_preference" && v == "rust")
            .collect();
        assert_eq!(rust_signals.len(), 1);
    }

    #[test]
    fn test_no_signals_in_generic_response() {
        let response = "Hello! How can I help you today?";
        let signals = extract_identity_signals(response);
        assert!(signals.is_empty());
    }

    #[test]
    fn test_multiple_interests_combined() {
        let response = "You can build a web API that serves machine learning model predictions using Python and FastAPI.";
        let signals = extract_identity_signals(response);
        assert!(signals.contains(&("work_domain".to_string(), "software_development".to_string())));
        assert!(signals.contains(&("tool_preference".to_string(), "python".to_string())));
        assert!(signals.contains(&("interest".to_string(), "artificial_intelligence".to_string())));
        assert!(signals.contains(&("interest".to_string(), "web_development".to_string())));
    }
}
