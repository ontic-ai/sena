//! Sentence boundary detection.
//!
//! Provides typed API for detecting sentence boundaries in text.
//! Supports streaming sentence extraction with abbreviation handling.

use tracing::trace;

/// Common abbreviations that should not trigger sentence boundaries.
const ABBREVIATIONS: &[&str] = &[
    "Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "Sr.", "Jr.", "vs.", "etc.", "i.e.", "e.g.", "Inc.",
    "Ltd.", "Co.", "St.", "Ave.", "Blvd.", "Rd.", "Dept.", "Ph.D.", "M.D.", "U.S.", "U.K.", "a.m.",
    "p.m.", "A.M.", "P.M.",
];

/// Detects a sentence boundary in a text buffer.
///
/// Returns `Some((completed_sentence, remaining_buffer))` if a boundary is found,
/// or `None` if the buffer does not contain a complete sentence.
///
/// Recognizes sentence endings: `.`, `!`, `?`, `...`, followed by whitespace.
/// Does not split on common abbreviations like `Dr.`, `Mr.`, `vs.`
///
/// # Examples
///
/// ```
/// use text::detect_sentence_boundary;
///
/// let result = detect_sentence_boundary("Hello world. Next sentence");
/// assert_eq!(result, Some(("Hello world.".to_string(), " Next sentence".to_string())));
///
/// let no_boundary = detect_sentence_boundary("Incomplete sentence");
/// assert_eq!(no_boundary, None);
/// ```
pub fn detect_sentence_boundary(buffer: &str) -> Option<(String, String)> {
    if buffer.is_empty() {
        return None;
    }

    let bytes = buffer.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let ch = bytes[i];

        // Check for potential sentence endings
        if ch == b'.' || ch == b'!' || ch == b'?' {
            // Check for ellipsis (... or …)
            let is_ellipsis = if ch == b'.' && i + 2 < bytes.len() {
                bytes[i + 1] == b'.' && bytes[i + 2] == b'.'
            } else {
                false
            };

            let end_pos = if is_ellipsis { i + 3 } else { i + 1 };

            // Must have whitespace after the punctuation to be a sentence boundary
            if end_pos < bytes.len() {
                let next_ch = bytes[end_pos];
                if next_ch.is_ascii_whitespace() {
                    // Check if this is an abbreviation
                    if !is_abbreviation(buffer, end_pos) {
                        let completed = buffer[..end_pos].to_string();
                        let remaining = buffer[end_pos..].to_string();
                        trace!(
                            completed_len = completed.len(),
                            remaining_len = remaining.len(),
                            "Sentence boundary detected"
                        );
                        return Some((completed, remaining));
                    }
                }
            } else if end_pos == bytes.len() {
                // Punctuation at end of buffer - might be incomplete
                // Only return if we're confident it's a complete sentence
                // (i.e., not an abbreviation)
                if !is_abbreviation(buffer, end_pos) {
                    let completed = buffer[..end_pos].to_string();
                    trace!(
                        completed_len = completed.len(),
                        "Sentence boundary detected at end of buffer"
                    );
                    return Some((completed, String::new()));
                }
            }

            // Skip past ellipsis if detected
            if is_ellipsis {
                i += 3;
                continue;
            }
        }

        i += 1;
    }

    None
}

/// Checks if the text ending at `end_pos` is a known abbreviation.
fn is_abbreviation(text: &str, end_pos: usize) -> bool {
    for abbr in ABBREVIATIONS {
        if end_pos >= abbr.len() {
            let start = end_pos - abbr.len();
            if let Some(slice) = text.get(start..end_pos)
                && slice == *abbr
            {
                return true;
            }
        }
    }
    false
}

/// Iterator that yields completed sentences from a streaming text buffer.
///
/// Accumulates text internally and emits sentences as they are detected.
/// Call `flush()` to retrieve any remaining text that doesn't form a complete sentence.
///
/// # Examples
///
/// ```
/// use text::SentenceBoundaryIterator;
///
/// let mut iter = SentenceBoundaryIterator::new();
/// iter.push("Hello world. ");
/// iter.push("How are you? ");
/// iter.push("Incomplete");
///
/// // Iterate through completed sentences
/// let first = iter.next();
/// assert_eq!(first, Some("Hello world.".to_string()));
///
/// let second = iter.next();
/// assert_eq!(second, Some(" How are you?".to_string()));
///
/// // No more complete sentences
/// assert_eq!(iter.next(), None);
///
/// // Get remaining incomplete text
/// let remaining = iter.flush();
/// assert_eq!(remaining, Some(" Incomplete".to_string()));
/// ```
#[derive(Debug, Clone)]
pub struct SentenceBoundaryIterator {
    buffer: String,
}

impl SentenceBoundaryIterator {
    /// Creates a new sentence boundary iterator.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Adds text to the internal buffer.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
    }

    /// Returns any remaining text in the buffer without a detected boundary.
    ///
    /// This is useful for handling incomplete sentences at the end of a stream.
    /// Returns `None` if the buffer is empty.
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            let remaining = std::mem::take(&mut self.buffer);
            Some(remaining)
        }
    }

    /// Returns the current buffer contents without consuming it.
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Clears the internal buffer.
    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

impl Default for SentenceBoundaryIterator {
    fn default() -> Self {
        Self::new()
    }
}

impl Iterator for SentenceBoundaryIterator {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.buffer.is_empty() {
            return None;
        }

        match detect_sentence_boundary(&self.buffer) {
            Some((completed, remaining)) => {
                self.buffer = remaining;
                Some(completed)
            }
            None => None,
        }
    }
}

/// Represents a detected sentence boundary in a text string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceBoundary {
    /// Byte offset where the sentence starts (inclusive).
    pub start: usize,
    /// Byte offset where the sentence ends (exclusive).
    pub end: usize,
}

impl SentenceBoundary {
    /// Creates a new sentence boundary.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Extracts the sentence text from the original input.
    ///
    /// Returns `None` if the boundary indices are invalid for the given text.
    pub fn extract<'a>(&self, text: &'a str) -> Option<&'a str> {
        text.get(self.start..self.end)
    }

    /// Returns the length of the sentence in bytes.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Returns whether this boundary represents an empty range.
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// Sentence splitter with configurable behavior.
///
/// Splits text into sentences using sentence boundary detection.
#[derive(Debug, Clone)]
pub struct SentenceSplitter {
    _private: (),
}

impl SentenceSplitter {
    /// Creates a new sentence splitter with default configuration.
    pub fn new() -> Self {
        trace!("SentenceSplitter::new()");
        Self { _private: () }
    }

    /// Splits text into sentence boundaries.
    ///
    /// Returns a vector of byte-indexed boundaries for each detected sentence.
    pub fn split(&self, text: &str) -> Vec<SentenceBoundary> {
        trace!(text_len = text.len(), "SentenceSplitter::split()");

        if text.is_empty() {
            return Vec::new();
        }

        let mut boundaries = Vec::new();
        let mut current_start = 0;
        let mut remaining = text;

        while !remaining.is_empty() {
            match detect_sentence_boundary(remaining) {
                Some((completed, _rest)) => {
                    let sentence_len = completed.len();
                    boundaries.push(SentenceBoundary::new(
                        current_start,
                        current_start + sentence_len,
                    ));
                    current_start += sentence_len;
                    remaining = &text[current_start..];
                }
                None => {
                    // No more boundaries found; treat remainder as final sentence if non-empty
                    if !remaining.trim().is_empty() {
                        boundaries.push(SentenceBoundary::new(current_start, text.len()));
                    }
                    break;
                }
            }
        }

        boundaries
    }

    /// Splits text and returns the extracted sentence strings.
    ///
    /// Returns a vector of sentence strings extracted from the input text.
    pub fn split_sentences<'a>(&self, text: &'a str) -> Vec<&'a str> {
        trace!(text_len = text.len(), "SentenceSplitter::split_sentences()");

        self.split(text)
            .iter()
            .filter_map(|boundary| boundary.extract(text))
            .collect()
    }
}

impl Default for SentenceSplitter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // =========================
    // SentenceBoundary tests
    // =========================

    #[test]
    fn boundary_new_creates_valid_boundary() {
        let boundary = SentenceBoundary::new(0, 10);
        assert_eq!(boundary.start, 0);
        assert_eq!(boundary.end, 10);
    }

    #[test]
    fn boundary_extract_returns_correct_slice() {
        let text = "Hello world. How are you?";
        let boundary = SentenceBoundary::new(0, 12);
        let extracted = boundary.extract(text);
        assert_eq!(extracted, Some("Hello world."));
    }

    #[test]
    fn boundary_extract_returns_none_for_invalid_range() {
        let text = "Hello";
        let boundary = SentenceBoundary::new(0, 100);
        let extracted = boundary.extract(text);
        assert_eq!(extracted, None);
    }

    #[test]
    fn boundary_len_returns_correct_length() {
        let boundary = SentenceBoundary::new(5, 15);
        assert_eq!(boundary.len(), 10);
    }

    #[test]
    fn boundary_is_empty_detects_empty_range() {
        let empty = SentenceBoundary::new(10, 10);
        assert!(empty.is_empty());

        let reversed = SentenceBoundary::new(10, 5);
        assert!(reversed.is_empty());

        let valid = SentenceBoundary::new(5, 10);
        assert!(!valid.is_empty());
    }

    #[test]
    fn boundary_extract_works_with_unicode() {
        let text = "Hello 世界! How are you?";
        let boundary = SentenceBoundary::new(0, 13); // "Hello 世界!" in UTF-8 bytes
        let extracted = boundary.extract(text);
        assert_eq!(extracted, Some("Hello 世界!"));
    }

    // =========================
    // detect_sentence_boundary tests
    // =========================

    #[test]
    fn detect_boundary_basic_period() {
        let result = detect_sentence_boundary("Hello world. Next");
        assert_eq!(
            result,
            Some(("Hello world.".to_string(), " Next".to_string()))
        );
    }

    #[test]
    fn detect_boundary_question_mark() {
        let result = detect_sentence_boundary("How are you? Fine");
        assert_eq!(
            result,
            Some(("How are you?".to_string(), " Fine".to_string()))
        );
    }

    #[test]
    fn detect_boundary_exclamation() {
        let result = detect_sentence_boundary("Stop! Go now");
        assert_eq!(result, Some(("Stop!".to_string(), " Go now".to_string())));
    }

    #[test]
    fn detect_boundary_ellipsis() {
        let result = detect_sentence_boundary("Wait... What happened?");
        assert_eq!(
            result,
            Some(("Wait...".to_string(), " What happened?".to_string()))
        );
    }

    #[test]
    fn detect_boundary_no_boundary_incomplete() {
        let result = detect_sentence_boundary("Incomplete sentence");
        assert_eq!(result, None);
    }

    #[test]
    fn detect_boundary_empty_buffer() {
        let result = detect_sentence_boundary("");
        assert_eq!(result, None);
    }

    #[test]
    fn detect_boundary_abbreviation_dr() {
        let result = detect_sentence_boundary("Dr. Smith is here. Next");
        // Should skip "Dr." and find the sentence ending at "here."
        assert_eq!(
            result,
            Some(("Dr. Smith is here.".to_string(), " Next".to_string()))
        );
    }

    #[test]
    fn detect_boundary_abbreviation_mr() {
        let result = detect_sentence_boundary("Mr. Jones called. What now?");
        assert_eq!(
            result,
            Some(("Mr. Jones called.".to_string(), " What now?".to_string()))
        );
    }

    #[test]
    fn detect_boundary_abbreviation_vs() {
        let result = detect_sentence_boundary("Team A vs. Team B won. Next game");
        assert_eq!(
            result,
            Some((
                "Team A vs. Team B won.".to_string(),
                " Next game".to_string()
            ))
        );
    }

    #[test]
    fn detect_boundary_abbreviation_etc() {
        let result = detect_sentence_boundary("Fruits, etc. are healthy. Good");
        assert_eq!(
            result,
            Some(("Fruits, etc. are healthy.".to_string(), " Good".to_string()))
        );
    }

    #[test]
    fn detect_boundary_period_at_end_no_whitespace() {
        let result = detect_sentence_boundary("Complete sentence.");
        assert_eq!(
            result,
            Some(("Complete sentence.".to_string(), "".to_string()))
        );
    }

    #[test]
    fn detect_boundary_abbreviation_at_end() {
        // If buffer ends with abbreviation, we should not detect a boundary
        let result = detect_sentence_boundary("Call Dr.");
        assert_eq!(result, None);
    }

    #[test]
    fn detect_boundary_multiple_sentences_returns_first() {
        let result = detect_sentence_boundary("First. Second. Third.");
        assert_eq!(
            result,
            Some(("First.".to_string(), " Second. Third.".to_string()))
        );
    }

    #[test]
    fn detect_boundary_whitespace_variations() {
        let result = detect_sentence_boundary("Sentence.  Double space");
        assert_eq!(
            result,
            Some(("Sentence.".to_string(), "  Double space".to_string()))
        );

        let result = detect_sentence_boundary("Sentence.\tTab after");
        assert_eq!(
            result,
            Some(("Sentence.".to_string(), "\tTab after".to_string()))
        );

        let result = detect_sentence_boundary("Sentence.\nNewline after");
        assert_eq!(
            result,
            Some(("Sentence.".to_string(), "\nNewline after".to_string()))
        );
    }

    // =========================
    // SentenceBoundaryIterator tests
    // =========================

    #[test]
    fn iterator_new_creates_empty() {
        let iter = SentenceBoundaryIterator::new();
        assert_eq!(iter.buffer(), "");
    }

    #[test]
    fn iterator_default_creates_empty() {
        let iter = SentenceBoundaryIterator::default();
        assert_eq!(iter.buffer(), "");
    }

    #[test]
    fn iterator_push_accumulates() {
        let mut iter = SentenceBoundaryIterator::new();
        iter.push("Hello ");
        iter.push("world.");
        assert_eq!(iter.buffer(), "Hello world.");
    }

    #[test]
    fn iterator_yields_completed_sentences() {
        let mut iter = SentenceBoundaryIterator::new();
        iter.push("Hello world. ");
        iter.push("How are you? ");

        let first = iter.next();
        assert_eq!(first, Some("Hello world.".to_string()));

        let second = iter.next();
        // The remaining buffer includes the leading whitespace after the previous sentence
        assert_eq!(second, Some(" How are you?".to_string()));

        let third = iter.next();
        assert_eq!(third, None);
    }

    #[test]
    fn iterator_flush_returns_remaining() {
        let mut iter = SentenceBoundaryIterator::new();
        iter.push("Complete. Incomplete");

        let completed = iter.next();
        assert_eq!(completed, Some("Complete.".to_string()));

        let remaining = iter.flush();
        assert_eq!(remaining, Some(" Incomplete".to_string()));
    }

    #[test]
    fn iterator_flush_empty_returns_none() {
        let mut iter = SentenceBoundaryIterator::new();
        assert_eq!(iter.flush(), None);
    }

    #[test]
    fn iterator_clear_empties_buffer() {
        let mut iter = SentenceBoundaryIterator::new();
        iter.push("Some text");
        iter.clear();
        assert_eq!(iter.buffer(), "");
    }

    #[test]
    fn iterator_handles_abbreviations() {
        let mut iter = SentenceBoundaryIterator::new();
        iter.push("Dr. Jones is here. Next sentence.");

        let first = iter.next();
        assert_eq!(first, Some("Dr. Jones is here.".to_string()));

        let second = iter.next();
        assert_eq!(second, Some(" Next sentence.".to_string()));
    }

    #[test]
    fn iterator_collect_all_sentences() {
        let mut iter = SentenceBoundaryIterator::new();
        iter.push("First. Second! Third? ");

        let sentences: Vec<String> = iter.collect();
        assert_eq!(sentences, vec!["First.", " Second!", " Third?"]);
    }

    #[test]
    fn iterator_streaming_scenario() {
        let mut iter = SentenceBoundaryIterator::new();

        // Simulate streaming input
        iter.push("The quick ");
        assert_eq!(iter.next(), None); // No complete sentence yet

        iter.push("brown fox. ");
        assert_eq!(iter.next(), Some("The quick brown fox.".to_string()));

        iter.push("Jumped over");
        assert_eq!(iter.next(), None); // Incomplete

        iter.push(" the lazy dog! ");
        assert_eq!(iter.next(), Some(" Jumped over the lazy dog!".to_string()));

        iter.push("End");
        let remaining = iter.flush();
        assert_eq!(remaining, Some(" End".to_string()));
    }

    // =========================
    // SentenceSplitter tests
    // =========================

    #[test]
    fn splitter_new_creates_instance() {
        let splitter = SentenceSplitter::new();
        assert_eq!(splitter._private, ());
    }

    #[test]
    fn splitter_default_creates_instance() {
        let splitter = SentenceSplitter::default();
        assert_eq!(splitter._private, ());
    }

    #[test]
    fn splitter_split_returns_empty_for_empty_text() {
        let splitter = SentenceSplitter::new();
        let boundaries = splitter.split("");
        assert_eq!(boundaries.len(), 0);
    }

    #[test]
    fn splitter_split_single_sentence() {
        let splitter = SentenceSplitter::new();
        let text = "Hello world.";
        let boundaries = splitter.split(text);

        assert_eq!(boundaries.len(), 1);
        assert_eq!(boundaries[0].start, 0);
        assert_eq!(boundaries[0].end, 12);
        assert_eq!(boundaries[0].extract(text), Some("Hello world."));
    }

    #[test]
    fn splitter_split_multiple_sentences() {
        let splitter = SentenceSplitter::new();
        let text = "First. Second! Third?";
        let boundaries = splitter.split(text);

        assert_eq!(boundaries.len(), 3);
        assert_eq!(boundaries[0].extract(text), Some("First."));
        assert_eq!(boundaries[1].extract(text), Some(" Second!"));
        assert_eq!(boundaries[2].extract(text), Some(" Third?"));
    }

    #[test]
    fn splitter_split_with_abbreviations() {
        let splitter = SentenceSplitter::new();
        let text = "Dr. Smith called. He said hello.";
        let boundaries = splitter.split(text);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].extract(text), Some("Dr. Smith called."));
        assert_eq!(boundaries[1].extract(text), Some(" He said hello."));
    }

    #[test]
    fn splitter_split_sentences_returns_strings() {
        let splitter = SentenceSplitter::new();
        let text = "One. Two! Three?";
        let sentences = splitter.split_sentences(text);

        assert_eq!(sentences, vec!["One.", " Two!", " Three?"]);
    }

    #[test]
    fn splitter_split_sentences_empty_text() {
        let splitter = SentenceSplitter::new();
        let sentences = splitter.split_sentences("");
        assert_eq!(sentences.len(), 0);
    }

    #[test]
    fn splitter_handles_incomplete_final_sentence() {
        let splitter = SentenceSplitter::new();
        let text = "Complete sentence. Incomplete";
        let boundaries = splitter.split(text);

        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].extract(text), Some("Complete sentence."));
        assert_eq!(boundaries[1].extract(text), Some(" Incomplete"));
    }

    #[test]
    fn splitter_whitespace_only_ignored() {
        let splitter = SentenceSplitter::new();
        let text = "Sentence.   ";
        let boundaries = splitter.split(text);

        // Only the sentence, not the trailing whitespace
        assert_eq!(boundaries.len(), 1);
        assert_eq!(boundaries[0].extract(text), Some("Sentence."));
    }

    // =========================
    // Property-based tests
    // =========================

    proptest! {
        #[test]
        fn prop_detect_boundary_never_panics(s in "\\PC*") {
            let _ = detect_sentence_boundary(&s);
        }

        #[test]
        fn prop_iterator_never_panics(s in "\\PC*") {
            let mut iter = SentenceBoundaryIterator::new();
            iter.push(&s);
            let _ = iter.next();
            let _ = iter.flush();
        }

        #[test]
        fn prop_splitter_never_panics(s in "\\PC*") {
            let splitter = SentenceSplitter::new();
            let _ = splitter.split(&s);
            let _ = splitter.split_sentences(&s);
        }

        #[test]
        fn prop_detect_boundary_preserves_content(s in "\\PC*") {
            if let Some((completed, remaining)) = detect_sentence_boundary(&s) {
                assert_eq!(completed + &remaining, s);
            }
        }

        #[test]
        fn prop_iterator_preserves_all_content(s in "\\PC{0,200}") {
            let mut iter = SentenceBoundaryIterator::new();
            iter.push(&s);

            let mut collected = String::new();
            while let Some(sentence) = iter.next() {
                collected.push_str(&sentence);
            }
            if let Some(remaining) = iter.flush() {
                collected.push_str(&remaining);
            }

            assert_eq!(collected, s);
        }

        #[test]
        fn prop_splitter_boundaries_non_overlapping(s in "\\PC{0,200}") {
            let splitter = SentenceSplitter::new();
            let boundaries = splitter.split(&s);

            for i in 0..boundaries.len().saturating_sub(1) {
                assert!(boundaries[i].end <= boundaries[i + 1].start);
            }
        }

        #[test]
        fn prop_splitter_boundaries_within_text(s in "\\PC{0,200}") {
            let splitter = SentenceSplitter::new();
            let boundaries = splitter.split(&s);

            for boundary in boundaries {
                assert!(boundary.start <= s.len());
                assert!(boundary.end <= s.len());
                assert!(boundary.start <= boundary.end);
            }
        }
    }
}
