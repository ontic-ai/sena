//! Type definitions for speech processing.

use serde::{Deserialize, Serialize};

/// STT backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SttBackendKind {
    /// Sherpa-ONNX streaming STT.
    Sherpa,
    /// NVIDIA Parakeet STT.
    Parakeet,
    /// OpenAI Whisper.
    Whisper,
}

impl SttBackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sherpa => "sherpa",
            Self::Parakeet => "parakeet",
            Self::Whisper => "whisper",
        }
    }
}

/// Events emitted by STT backends during processing.
#[derive(Debug, Clone)]
pub enum SttEvent {
    /// A word was recognized.
    Word { text: String, confidence: f32 },

    /// Transcription completed for current audio segment.
    Completed { text: String, confidence: f32 },

    /// Backend is listening for audio.
    Listening,

    /// Backend stopped listening.
    Stopped,

    /// An error occurred during processing.
    Error { reason: String },
}

/// Audio stream produced by TTS synthesis.
#[derive(Debug, Clone, zeroize::ZeroizeOnDrop)]
pub struct AudioStream {
    /// PCM samples (f32, mono, sample rate determined by backend).
    pub samples: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl AudioStream {
    pub fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            samples,
            sample_rate,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.samples.len() as u64 * 1000) / self.sample_rate as u64
    }
}

/// Transcription result with text and confidence.
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// Transcribed text.
    pub text: String,
    /// Confidence score [0.0, 1.0].
    pub confidence: f32,
}

/// Pending sentence in TTS queue, awaiting synthesis or playback.
#[derive(Debug, Clone)]
pub struct PendingSentence {
    /// Sentence text to synthesize.
    pub text: String,
    /// Sentence index for ordering.
    pub sentence_index: u32,
    /// Synthesized audio (None if synthesis not yet complete).
    pub audio: Option<Vec<u8>>,
    /// Whether synthesis is complete and audio is ready for playback.
    pub ready: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_backend_kind_as_str() {
        assert_eq!(SttBackendKind::Sherpa.as_str(), "sherpa");
        assert_eq!(SttBackendKind::Parakeet.as_str(), "parakeet");
        assert_eq!(SttBackendKind::Whisper.as_str(), "whisper");
    }

    #[test]
    fn audio_stream_duration_calculation() {
        let stream = AudioStream::new(vec![0.0; 16000], 16000);
        assert_eq!(stream.duration_ms(), 1000);
    }

    #[test]
    fn audio_stream_empty_check() {
        let stream = AudioStream::new(vec![], 16000);
        assert!(stream.is_empty());
    }

    #[test]
    fn transcription_result_constructs() {
        let result = TranscriptionResult {
            text: "hello world".to_string(),
            confidence: 0.95,
        };
        assert_eq!(result.text, "hello world");
        assert_eq!(result.confidence, 0.95);
    }

    #[test]
    fn pending_sentence_constructs() {
        let sentence = PendingSentence {
            text: "Test sentence".to_string(),
            sentence_index: 5,
            audio: Some(vec![1, 2, 3, 4]),
            ready: true,
        };
        assert_eq!(sentence.text, "Test sentence");
        assert_eq!(sentence.sentence_index, 5);
        assert!(sentence.ready);
        assert!(sentence.audio.is_some());
    }

    #[test]
    fn pending_sentence_not_ready() {
        let sentence = PendingSentence {
            text: "Pending".to_string(),
            sentence_index: 0,
            audio: None,
            ready: false,
        };
        assert!(!sentence.ready);
        assert!(sentence.audio.is_none());
    }
}
