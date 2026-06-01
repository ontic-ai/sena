use serde::{Deserialize, Serialize};

pub const DEFAULT_STT_SAMPLE_RATE_HZ: u32 = 16_000;
pub const DEFAULT_STT_BUFFER_DURATION_MS: u64 = 100;
pub const DEFAULT_STT_SILENCE_DURATION_MS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListenMode {
    AlwaysListen,
    PushToTalk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechConfig {
    pub stt_sample_rate_hz: u32,
    pub stt_buffer_duration_ms: u64,
    pub stt_silence_duration_ms: u64,
    pub always_listen: bool,
    pub listen_mode: ListenMode,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            stt_sample_rate_hz: DEFAULT_STT_SAMPLE_RATE_HZ,
            stt_buffer_duration_ms: DEFAULT_STT_BUFFER_DURATION_MS,
            stt_silence_duration_ms: DEFAULT_STT_SILENCE_DURATION_MS,
            always_listen: true,
            listen_mode: ListenMode::AlwaysListen,
        }
    }
}

pub fn normalize_reply_for_tts(input: &str) -> Option<String> {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}
