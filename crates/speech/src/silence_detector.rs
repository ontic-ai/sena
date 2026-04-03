//! Voice activity detection and silence-gated transcription triggering.

use std::time::Instant;

/// Encapsulates voice activity detection and silence-gated accumulation.
///
/// Each audio processing mode (always-listening, listen-mode) should own
/// its own `SilenceDetector` instance so their state cannot cross-contaminate.
pub(crate) struct SilenceDetector {
    accumulated_samples: Vec<f32>,
    last_voice_activity: Option<Instant>,
    speech_started: bool,
    energy_threshold: f32,
    silence_duration_secs: f32,
}

impl SilenceDetector {
    pub(crate) fn new(energy_threshold: f32, silence_duration_secs: f32) -> Self {
        Self {
            accumulated_samples: Vec::new(),
            last_voice_activity: None,
            speech_started: false,
            energy_threshold,
            silence_duration_secs,
        }
    }

    /// Feed an audio buffer.  Returns `Some(accumulated_samples)` when speech
    /// followed by silence has been detected (ready for transcription).
    /// Returns `None` otherwise (still accumulating or silence without prior speech).
    pub(crate) fn feed(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Option<crate::AudioBuffer> {
        let rms = calculate_rms(samples);
        let is_voice = rms > self.energy_threshold;
        let now = Instant::now();

        if is_voice {
            if !self.speech_started {
                self.speech_started = true;
                self.accumulated_samples.clear();
            }
            self.accumulated_samples.extend_from_slice(samples);
            self.last_voice_activity = Some(now);
            None
        } else if self.speech_started {
            if let Some(last_voice) = self.last_voice_activity {
                let silence_secs = now.duration_since(last_voice).as_secs_f32();
                if silence_secs >= self.silence_duration_secs
                    && !self.accumulated_samples.is_empty()
                {
                    let buffer = crate::AudioBuffer {
                        samples: self.accumulated_samples.clone(),
                        sample_rate,
                        channels,
                    };
                    self.reset();
                    Some(buffer)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Clear all accumulated state.
    pub(crate) fn reset(&mut self) {
        self.accumulated_samples.clear();
        self.speech_started = false;
        self.last_voice_activity = None;
    }
}

pub(crate) fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_detector_no_speech_returns_none() {
        let mut det = SilenceDetector::new(0.01, 1.5);
        let silence = vec![0.001; 160];
        assert!(det.feed(&silence, 16_000, 1).is_none());
    }

    #[test]
    fn silence_detector_speech_then_silence_returns_buffer() {
        let mut det = SilenceDetector::new(0.01, 0.0); // 0s silence threshold for instant trigger
                                                       // Feed speech
        let speech = vec![0.5; 160];
        assert!(det.feed(&speech, 16_000, 1).is_none());
        // Feed silence — should trigger since silence_duration_secs = 0.0
        let silence = vec![0.001; 160];
        let result = det.feed(&silence, 16_000, 1);
        assert!(result.is_some());
        let buf = result.expect("feed should return buffer after speech+silence");
        assert_eq!(buf.samples.len(), 160); // only speech samples
    }

    #[test]
    fn silence_detector_reset_clears_state() {
        let mut det = SilenceDetector::new(0.01, 1.5);
        let speech = vec![0.5; 160];
        det.feed(&speech, 16_000, 1);
        assert!(det.speech_started);
        det.reset();
        assert!(!det.speech_started);
        assert!(det.accumulated_samples.is_empty());
        assert!(det.last_voice_activity.is_none());
    }

    #[test]
    fn two_detectors_are_independent() {
        let mut det1 = SilenceDetector::new(0.01, 0.0);
        let mut det2 = SilenceDetector::new(0.01, 0.0);
        let speech = vec![0.5; 160];
        det1.feed(&speech, 16_000, 1);
        assert!(det1.speech_started);
        assert!(!det2.speech_started);
        // Reset det1 does not affect det2
        det1.reset();
        det2.feed(&speech, 16_000, 1);
        assert!(!det1.speech_started);
        assert!(det2.speech_started);
    }
}
