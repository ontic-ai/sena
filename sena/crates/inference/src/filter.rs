//! Output filter for inference token streams.
//!
//! Strips markdown and formatting artifacts from generated text before
//! the text is passed to TTS synthesis. This ensures TTS receives clean,
//! speakable text without markdown symbols that would be read aloud literally.

/// Filters inference output text for downstream consumers such as TTS.
pub struct OutputFilter;

impl OutputFilter {
    /// Strip markdown formatting from generated text.
    ///
    /// Removes common markdown syntax that would be read aloud verbatim by TTS
    /// (e.g., `**bold**`, `_italic_`, backtick code blocks, header `#` symbols).
    ///
    /// Does not alter punctuation, sentence boundaries, or semantic content.
    pub fn apply(text: &str) -> String {
        // Strip fenced code blocks first (triple backtick)
        let text = text.replace("```", "");
        // Strip inline code (single backtick)
        let text = text.replace('`', "");
        // Strip bold (**text**)
        let text = text.replace("**", "");
        // Strip italic (*text* or _text_) — asterisk last after bold
        let text = text.replace('*', "");
        // Strip bold/italic underscores
        let text = text.replace("__", "");
        let text = text.replace('_', "");
        // Strip markdown headers (# at start of word)
        let text = text.replace("### ", "");
        let text = text.replace("## ", "");
        let text = text.replace("# ", "");
        // Strip horizontal rules
        let text = text.replace("---", "");
        // Collapse excess whitespace without changing sentence structure
        let text: String = text.lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
        // Collapse multiple spaces
        let mut output = String::with_capacity(text.len());
        let mut prev_space = false;
        for ch in text.chars() {
            if ch == ' ' {
                if !prev_space {
                    output.push(ch);
                }
                prev_space = true;
            } else {
                output.push(ch);
                prev_space = false;
            }
        }
        output.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_strips_bold_markdown() {
        let result = OutputFilter::apply("**Hello** world");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn filter_strips_italics() {
        let result = OutputFilter::apply("_Hello_ world");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn filter_strips_inline_code() {
        let result = OutputFilter::apply("Use `cargo build` to compile");
        assert_eq!(result, "Use cargo build to compile");
    }

    #[test]
    fn filter_strips_headers() {
        let result = OutputFilter::apply("## Introduction");
        assert_eq!(result, "Introduction");
    }

    #[test]
    fn filter_plain_text_unchanged() {
        let result = OutputFilter::apply("Hello, this is plain text.");
        assert_eq!(result, "Hello, this is plain text.");
    }

    #[test]
    fn filter_empty_string() {
        let result = OutputFilter::apply("");
        assert_eq!(result, "");
    }
}
