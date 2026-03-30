//! Chat template wrapper for instruction-tuned models.
//!
//! Wraps user prompts in model-specific role markers so that instruction-tuned
//! models receive properly formatted input instead of raw text.
//!
//! This module detects the appropriate template based on model name and applies
//! the correct formatting. It does NOT add any persona or system instructions —
//! it only wraps the user-supplied prompt in role markers.

/// Chat template format for instruction-tuned models.
///
/// Each variant corresponds to a specific model family's expected input format.
/// `Raw` is the safe default when no template is detected — it passes text through
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatTemplate {
    /// Gemma format: `<start_of_turn>user\n{PROMPT}<end_of_turn>\n<start_of_turn>model\n`
    Gemma,
    /// Mistral/Mixtral format: `[INST] {PROMPT} [/INST]`
    Mistral,
    /// Llama 2 format: `<s>[INST] {PROMPT} [/INST]`
    Llama2,
    /// Alpaca format: `### Instruction:\n{PROMPT}\n\n### Response:`
    Alpaca,
    /// Raw passthrough — no template applied (safe default)
    Raw,
}

impl ChatTemplate {
    /// Detect the appropriate chat template from a model name.
    ///
    /// Uses case-insensitive substring matching:
    /// - "gemma" → Gemma
    /// - "mistral" or "mixtral" → Mistral
    /// - "llama" → Llama2
    /// - "alpaca" → Alpaca
    /// - anything else → Raw (safe default)
    pub(crate) fn detect_from_model_name(name: &str) -> Self {
        let name_lower = name.to_lowercase();

        if name_lower.contains("gemma") {
            ChatTemplate::Gemma
        } else if name_lower.contains("mistral") || name_lower.contains("mixtral") {
            ChatTemplate::Mistral
        } else if name_lower.contains("llama") {
            ChatTemplate::Llama2
        } else if name_lower.contains("alpaca") {
            ChatTemplate::Alpaca
        } else {
            ChatTemplate::Raw
        }
    }

    /// Wrap the user prompt in the template's role markers.
    ///
    /// Returns a formatted string with the user prompt wrapped in the appropriate
    /// role markers for the model. Does NOT add any persona or system instructions —
    /// only wraps the prompt text.
    ///
    /// For `Raw` template, returns the prompt unchanged.
    pub(crate) fn wrap(&self, user_prompt: &str) -> String {
        match self {
            ChatTemplate::Gemma => {
                format!(
                    "<start_of_turn>user\n{}<end_of_turn>\n<start_of_turn>model\n",
                    user_prompt
                )
            }
            ChatTemplate::Mistral => {
                format!("[INST] {} [/INST]", user_prompt)
            }
            ChatTemplate::Llama2 => {
                format!("<s>[INST] {} [/INST]", user_prompt)
            }
            ChatTemplate::Alpaca => {
                format!("### Instruction:\n{}\n\n### Response:", user_prompt)
            }
            ChatTemplate::Raw => user_prompt.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_gemma_template() {
        assert_eq!(
            ChatTemplate::detect_from_model_name("gemma-2-9b-it"),
            ChatTemplate::Gemma
        );
        assert_eq!(
            ChatTemplate::detect_from_model_name("GEMMA-7B"),
            ChatTemplate::Gemma
        );
    }

    #[test]
    fn detects_mistral_template() {
        assert_eq!(
            ChatTemplate::detect_from_model_name("mistral-7b-instruct"),
            ChatTemplate::Mistral
        );
        assert_eq!(
            ChatTemplate::detect_from_model_name("mixtral-8x7b"),
            ChatTemplate::Mistral
        );
    }

    #[test]
    fn detects_llama_template() {
        assert_eq!(
            ChatTemplate::detect_from_model_name("llama-2-7b-chat"),
            ChatTemplate::Llama2
        );
        assert_eq!(
            ChatTemplate::detect_from_model_name("meta-llama-3"),
            ChatTemplate::Llama2
        );
    }

    #[test]
    fn detects_alpaca_template() {
        assert_eq!(
            ChatTemplate::detect_from_model_name("alpaca-7b"),
            ChatTemplate::Alpaca
        );
    }

    #[test]
    fn defaults_to_raw_for_unknown_models() {
        assert_eq!(
            ChatTemplate::detect_from_model_name("gpt2"),
            ChatTemplate::Raw
        );
        assert_eq!(
            ChatTemplate::detect_from_model_name("unknown-model"),
            ChatTemplate::Raw
        );
    }

    #[test]
    fn gemma_template_wraps_correctly() {
        let template = ChatTemplate::Gemma;
        let wrapped = template.wrap("What is Rust?");
        assert_eq!(
            wrapped,
            "<start_of_turn>user\nWhat is Rust?<end_of_turn>\n<start_of_turn>model\n"
        );
    }

    #[test]
    fn mistral_template_wraps_correctly() {
        let template = ChatTemplate::Mistral;
        let wrapped = template.wrap("Explain async Rust");
        assert_eq!(wrapped, "[INST] Explain async Rust [/INST]");
    }

    #[test]
    fn llama2_template_wraps_correctly() {
        let template = ChatTemplate::Llama2;
        let wrapped = template.wrap("Tell me about actors");
        assert_eq!(wrapped, "<s>[INST] Tell me about actors [/INST]");
    }

    #[test]
    fn alpaca_template_wraps_correctly() {
        let template = ChatTemplate::Alpaca;
        let wrapped = template.wrap("Write a function");
        assert_eq!(
            wrapped,
            "### Instruction:\nWrite a function\n\n### Response:"
        );
    }

    #[test]
    fn raw_template_returns_unchanged() {
        let template = ChatTemplate::Raw;
        let prompt = "This is a raw prompt";
        let wrapped = template.wrap(prompt);
        assert_eq!(wrapped, prompt);
    }

    #[test]
    fn wrap_preserves_multiline_prompts() {
        let template = ChatTemplate::Mistral;
        let multiline = "First line\nSecond line\nThird line";
        let wrapped = template.wrap(multiline);
        assert_eq!(
            wrapped,
            "[INST] First line\nSecond line\nThird line [/INST]"
        );
    }

    #[test]
    fn wrap_handles_empty_prompt() {
        let template = ChatTemplate::Gemma;
        let wrapped = template.wrap("");
        assert_eq!(
            wrapped,
            "<start_of_turn>user\n<end_of_turn>\n<start_of_turn>model\n"
        );
    }
}
