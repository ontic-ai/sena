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
#[derive(Debug, Clone)]
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
}
