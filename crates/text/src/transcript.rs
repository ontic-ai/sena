//! Transcript processing utilities — prompt construction and text normalization.
//!
//! These helpers are consumed by the inference crate to build task-directed
//! prompts from live transcript data. All content is derived from caller-supplied
//! data — no static persona strings.

/// Build a transcript cleanup prompt from raw speech-to-text output.
///
/// The returned string asks the LLM to convert raw transcription into properly
/// formatted text with correct capitaliation and punctuation.
///
/// # Arguments
/// * `raw` — the raw transcript text from the STT backend (pre-trimmed or not).
pub fn transcript_cleanup_prompt(raw: &str) -> String {
    format!(
        "Fix the following speech-to-text transcript.\n\
         Correct acoustic confusions and misheard words (for example: \
         \"off of it\" might be \"alphabet\" if the context suggests it).\n\
         Add proper capitalization and punctuation.\n\
         Preserve the original meaning exactly.\n\
         Return only the corrected text, nothing else.\n\n{}",
        raw.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_cleanup_prompt_includes_raw_text() {
        let raw = "the quick brown fox";
        let prompt = transcript_cleanup_prompt(raw);
        assert!(prompt.contains(raw), "prompt should include the raw text");
        assert!(prompt.contains("speech-to-text"), "prompt should describe the task");
    }

    #[test]
    fn transcript_cleanup_prompt_trims_whitespace() {
        let raw = "  the quick brown fox  ";
        let prompt = transcript_cleanup_prompt(raw);
        assert!(prompt.contains("the quick brown fox"), "should trim input");
        assert!(!prompt.contains("  the quick brown"), "should not include leading spaces");
    }
}
