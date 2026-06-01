use serde::{Deserialize, Serialize};
use speech::SpeechConfig;

pub const HARDCODED_MODEL: &str = "qwen2.5:7b-instruct";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub model: String,
    pub attach_cli: bool,
    pub speech: SpeechConfig,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            model: HARDCODED_MODEL.to_owned(),
            attach_cli: cfg!(debug_assertions),
            speech: SpeechConfig::default(),
        }
    }
}
