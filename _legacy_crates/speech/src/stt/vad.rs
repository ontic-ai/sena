//! Voice activity detection wrapper.
//!
//! This module provides a clean public API over the existing SilenceDetector
//! for use in STT pipeline integration.

use crate::silence_detector::SilenceDetector;
use crate::AudioBuffer;

/// Voice activity detector for speech-silence segmentation.
///
/// Wraps the existing `SilenceDetector` with a cleaner API for integration
/// into the Whisper pipeline.
pub struct VoiceActivityDetector {
    detector: SilenceDetector,
}

impl VoiceActivityDetector {
    /// Create a new voice activity detector.
    ///
    /// # Arguments
    /// - `energy_threshold`: RMS energy threshold for voice detection (e.g., 0.01)
    /// - `silence_duration_secs`: Duration of silence required to trigger transcription
    pub fn new(energy_threshold: f32, silence_duration_secs: f32) -> Self {
        Self {
            detector: SilenceDetector::new(energy_threshold, silence_duration_secs),
        }
    }

    /// Feed an audio buffer and get accumulated speech if ready.
    ///
    /// Returns `Some(AudioBuffer)` when speech followed by sufficient silence
    /// has been detected. Returns `None` otherwise (still accumulating or
    /// insufficient speech).
    pub fn feed(&mut self, buffer: &AudioBuffer) -> Option<AudioBuffer> {
        self.detector
            .feed(&buffer.samples, buffer.sample_rate, buffer.channels)
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.detector.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_wraps_silence_detector_correctly() {
        let mut vad = VoiceActivityDetector::new(0.01, 0.0);

        // Feed 1 second of speech at 16kHz
        let speech = AudioBuffer {
            samples: vec![0.5_f32; 16_000],
            sample_rate: 16_000,
            channels: 1,
        };
        assert!(vad.feed(&speech).is_none());

        // Feed silence — should trigger (0s threshold)
        let silence = AudioBuffer {
            samples: vec![0.001_f32; 16_000],
            sample_rate: 16_000,
            channels: 1,
        };
        let result = vad.feed(&silence);
        assert!(result.is_some());
        assert_eq!(result.unwrap().samples.len(), 16_000);
    }

    #[test]
    fn vad_reset_clears_state() {
        let mut vad = VoiceActivityDetector::new(0.01, 1.5);

        let speech = AudioBuffer {
            samples: vec![0.5_f32; 16_000],
            sample_rate: 16_000,
            channels: 1,
        };
        vad.feed(&speech);

        vad.reset();

        // After reset, state should be clear
        let silence = AudioBuffer {
            samples: vec![0.001_f32; 16_000],
            sample_rate: 16_000,
            channels: 1,
        };
        assert!(vad.feed(&silence).is_none());
    }
}
