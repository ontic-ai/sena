//! STT subsystem internals.
//!
//! This module houses the Whisper pipeline, voice activity detection,
//! and confidence scoring utilities for speech-to-text processing.

pub mod confidence;
pub mod pipeline;
pub mod vad;

pub use confidence::{confidence_tier, log_prob_to_confidence, ConfidenceTier};
pub use pipeline::{TranscriptionSegment, WhisperPipeline};
pub use vad::VoiceActivityDetector;
