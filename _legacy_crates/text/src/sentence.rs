/// Detect the first sentence boundary in `buffer`.
///
/// Returns `Some((sentence, remainder))` where:
/// - `sentence` is the complete sentence including its terminal character.
/// - `remainder` is everything after the boundary (may be empty).
///
/// Returns `None` when no complete sentence boundary is detected and neither
/// threshold forces a split.
///
/// # Boundary rules (in priority order)
///
/// 1. **Hard boundary**: `.`, `?`, or `!` followed by whitespace or end-of-string.
/// 2. **Soft boundary**: `;` followed by whitespace.
/// 3. **Comma boundary**: `,` followed by whitespace, only when `buffer.len() > max_buffer_chars`.
/// 4. **Max sentence cap**: when `buffer.len() > max_sentence_chars`, split at the nearest
///    whitespace before the threshold (or at the threshold if no whitespace found).
///
/// # Parameters
///
/// - `buffer`: current accumulation buffer (running token stream so far).
/// - `max_buffer_chars`: comma threshold; a comma boundary only fires when buffer exceeds this.
/// - `max_sentence_chars`: hard cap; forces a split when buffer exceeds this length.
pub fn detect_sentence_boundary(
    buffer: &str,
    max_buffer_chars: usize,
    max_sentence_chars: usize,
) -> Option<(String, String)> {
    // Empty or whitespace-only buffer returns None
    if buffer.trim().is_empty() {
        return None;
    }

    let chars: Vec<char> = buffer.chars().collect();
    let len = chars.len();

    // Priority 1: Hard boundary (., ?, !) followed by whitespace or end-of-string
    for i in 0..len {
        let ch = chars[i];
        if ch == '.' || ch == '?' || ch == '!' {
            // Check if followed by whitespace or end-of-string
            if i + 1 >= len {
                // End of string
                let sentence = buffer.to_string();
                return Some((sentence, String::new()));
            } else if chars[i + 1].is_whitespace() {
                // Followed by whitespace
                let sentence: String = chars[..=i].iter().collect();
                let remainder: String = chars[i + 1..].iter().collect();
                return Some((sentence, remainder.trim_start().to_string()));
            }
        }
    }

    // Priority 2: Soft boundary (;) followed by whitespace
    for i in 0..len {
        let ch = chars[i];
        if ch == ';' && i + 1 < len && chars[i + 1].is_whitespace() {
            let sentence: String = chars[..=i].iter().collect();
            let remainder: String = chars[i + 1..].iter().collect();
            return Some((sentence, remainder.trim_start().to_string()));
        }
    }

    // Priority 3: Comma boundary (,) followed by whitespace, only when buffer exceeds max_buffer_chars
    if buffer.len() > max_buffer_chars {
        for i in 0..len {
            let ch = chars[i];
            if ch == ',' && i + 1 < len && chars[i + 1].is_whitespace() {
                let sentence: String = chars[..=i].iter().collect();
                let remainder: String = chars[i + 1..].iter().collect();
                return Some((sentence, remainder.trim_start().to_string()));
            }
        }
    }

    // Priority 4: Max sentence cap
    if buffer.len() > max_sentence_chars {
        // Find the last whitespace before max_sentence_chars
        let mut split_pos = None;
        for (i, ch) in chars.iter().enumerate().take(max_sentence_chars.min(len)) {
            if ch.is_whitespace() {
                split_pos = Some(i);
            }
        }

        if let Some(pos) = split_pos {
            // Split at the whitespace
            let sentence: String = chars[..pos].iter().collect();
            let remainder: String = chars[pos..].iter().collect();
            return Some((sentence, remainder.trim_start().to_string()));
        } else {
            // No whitespace found before threshold, split at max_sentence_chars
            let split_at = max_sentence_chars.min(len);
            let sentence: String = chars[..split_at].iter().collect();
            let remainder: String = chars[split_at..].iter().collect();
            return Some((sentence, remainder.trim_start().to_string()));
        }
    }

    // No boundary found
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_returns_none() {
        assert_eq!(detect_sentence_boundary("", 100, 200), None);
    }

    #[test]
    fn whitespace_only_buffer_returns_none() {
        assert_eq!(detect_sentence_boundary("   ", 100, 200), None);
        assert_eq!(detect_sentence_boundary("\n\t  ", 100, 200), None);
    }

    #[test]
    fn buffer_with_no_punctuation_returns_none() {
        assert_eq!(detect_sentence_boundary("Hello world", 100, 200), None);
    }

    #[test]
    fn period_boundary_with_following_text() {
        let result = detect_sentence_boundary("Hello world. This", 100, 200);
        assert_eq!(
            result,
            Some(("Hello world.".to_string(), "This".to_string()))
        );
    }

    #[test]
    fn question_mark_boundary() {
        let result = detect_sentence_boundary("Are you sure? Yes", 100, 200);
        assert_eq!(
            result,
            Some(("Are you sure?".to_string(), "Yes".to_string()))
        );
    }

    #[test]
    fn exclamation_boundary() {
        let result = detect_sentence_boundary("Watch out! The", 100, 200);
        assert_eq!(result, Some(("Watch out!".to_string(), "The".to_string())));
    }

    #[test]
    fn period_at_end_of_string() {
        let result = detect_sentence_boundary("Hello world.", 100, 200);
        assert_eq!(result, Some(("Hello world.".to_string(), "".to_string())));
    }

    #[test]
    fn semicolon_soft_boundary() {
        let result = detect_sentence_boundary("First part; second", 100, 200);
        assert_eq!(
            result,
            Some(("First part;".to_string(), "second".to_string()))
        );
    }

    #[test]
    fn comma_threshold_not_exceeded_returns_none() {
        // Buffer length is 10, max_buffer_chars is 10, so NOT over threshold
        let result = detect_sentence_boundary("hello, world", 12, 200);
        assert_eq!(result, None);
    }

    #[test]
    fn comma_threshold_exceeded_triggers_split() {
        // Buffer length is 12, max_buffer_chars is 10, so over threshold
        let result = detect_sentence_boundary("hello, world", 10, 200);
        assert_eq!(result, Some(("hello,".to_string(), "world".to_string())));
    }

    #[test]
    fn max_sentence_cap_with_whitespace_before_threshold() {
        // Buffer length is 20, max_sentence_chars is 15, split at last whitespace before 15
        let result = detect_sentence_boundary("hello world testing", 100, 15);
        // Last whitespace before position 15 is at position 11 (after "world")
        assert_eq!(
            result,
            Some(("hello world".to_string(), "testing".to_string()))
        );
    }

    #[test]
    fn max_sentence_cap_no_whitespace_before_threshold() {
        // Buffer length is 20, max_sentence_chars is 10, no whitespace in first 10 chars
        let result = detect_sentence_boundary("helloworld testing", 100, 10);
        assert_eq!(
            result,
            Some(("helloworld".to_string(), "testing".to_string()))
        );
    }

    #[test]
    fn buffer_ending_mid_word_no_punctuation_returns_none() {
        let result = detect_sentence_boundary("hello wor", 100, 200);
        assert_eq!(result, None);
    }

    #[test]
    fn period_not_followed_by_whitespace_not_a_boundary() {
        // "www.google.com more" - periods in URL are NOT boundaries
        let result = detect_sentence_boundary("www.google.com more", 100, 200);
        // The first two periods are not followed by whitespace, so not boundaries
        // No valid boundary found
        assert_eq!(result, None);
    }

    #[test]
    fn multiple_whitespace_after_boundary_trimmed() {
        let result = detect_sentence_boundary("Hello.   World", 100, 200);
        assert_eq!(result, Some(("Hello.".to_string(), "World".to_string())));
    }

    #[test]
    fn hard_boundary_takes_priority_over_soft() {
        // Both period and semicolon present, period comes first
        let result = detect_sentence_boundary("Hello. world; test", 100, 200);
        assert_eq!(
            result,
            Some(("Hello.".to_string(), "world; test".to_string()))
        );
    }

    #[test]
    fn soft_boundary_takes_priority_over_comma() {
        // Both semicolon and comma present with buffer over threshold
        let result = detect_sentence_boundary("First; second, third", 10, 200);
        assert_eq!(
            result,
            Some(("First;".to_string(), "second, third".to_string()))
        );
    }

    #[test]
    fn question_mark_at_end() {
        let result = detect_sentence_boundary("Is this working?", 100, 200);
        assert_eq!(
            result,
            Some(("Is this working?".to_string(), "".to_string()))
        );
    }

    #[test]
    fn exclamation_at_end() {
        let result = detect_sentence_boundary("Wow!", 100, 200);
        assert_eq!(result, Some(("Wow!".to_string(), "".to_string())));
    }

    #[test]
    fn semicolon_at_end_no_whitespace_returns_none() {
        // Semicolon must be followed by whitespace to be a boundary
        let result = detect_sentence_boundary("Test;", 100, 200);
        assert_eq!(result, None);
    }

    #[test]
    fn comma_at_end_no_whitespace_returns_none() {
        let result = detect_sentence_boundary("Test,", 100, 200);
        assert_eq!(result, None);
    }

    #[test]
    fn max_cap_exact_boundary() {
        // Buffer exactly exceeds cap
        let result = detect_sentence_boundary("hello ", 100, 5);
        // Buffer length is 6, max_sentence_chars is 5
        // Last whitespace before position 5 is at position 5 (but we need before, not at)
        // Actually position 5 is the space, so last whitespace before 5 doesn't exist
        // Split at position 5
        assert_eq!(result, Some(("hello".to_string(), "".to_string())));
    }
}
