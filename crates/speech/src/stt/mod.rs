//! STT subsystem internals.
//!
//! This module houses voice activity detection and confidence scoring utilities
//! for speech-to-text processing. The Whisper pipeline is now isolated in the stt-worker binary
//! to avoid GGML symbol conflicts with llama.cpp.

pub mod confidence;
// pipeline module removed — WhisperPipeline now lives in stt-worker binary
pub mod vad;

pub use confidence::{confidence_tier, log_prob_to_confidence, ConfidenceTier};
pub use vad::VoiceActivityDetector;
