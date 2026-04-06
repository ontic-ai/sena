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
    /// Minimum total voice duration (seconds) required before the accumulated
    /// buffer is eligible for transcription. Prevents brief sounds (key clicks,
    /// coughs, desk knocks) from triggering spurious transcription. With 3-second
    /// chunks this guards against a single noisy chunk; with shorter chunks
    /// (Phase 7 streaming) it is the primary guard against transient noise.
    min_speech_duration_secs: f32,
    /// Tracks total accumulated voice time across all voice chunks in this session.
    total_voice_duration_secs: f32,
}

impl SilenceDetector {
    pub(crate) fn new(energy_threshold: f32, silence_duration_secs: f32) -> Self {
        Self {
            accumulated_samples: Vec::new(),
            last_voice_activity: None,
            speech_started: false,
            energy_threshold,
            silence_duration_secs,
            min_speech_duration_secs: 0.5,
            total_voice_duration_secs: 0.0,
        }
    }

    /// Feed an audio buffer.  Returns `Some(accumulated_samples)` when speech
    /// followed by silence has been detected (ready for transcription).
    /// Returns `None` otherwise (still accumulating, insufficient speech, or
    /// silence without prior speech).
    pub(crate) fn feed(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Option<crate::AudioBuffer> {
        let rms = calculate_rms(samples);
        let is_voice = rms > self.energy_threshold;
        let now = Instant::now();
        let chunk_duration = samples.len() as f32 / (sample_rate as f32 * channels as f32);

        if is_voice {
            if !self.speech_started {
                self.speech_started = true;
                self.accumulated_samples.clear();
                self.total_voice_duration_secs = 0.0;
            }
            self.accumulated_samples.extend_from_slice(samples);
            self.total_voice_duration_secs += chunk_duration;
            self.last_voice_activity = Some(now);
            None
        } else if self.speech_started {
            if let Some(last_voice) = self.last_voice_activity {
                let silence_secs = now.duration_since(last_voice).as_secs_f32();
                if silence_secs >= self.silence_duration_secs
                    && !self.accumulated_samples.is_empty()
                    && self.total_voice_duration_secs >= self.min_speech_duration_secs
                {
                    let buffer = crate::AudioBuffer {
                        samples: self.accumulated_samples.clone(),
                        sample_rate,
                        channels,
                    };
                    self.reset();
                    Some(buffer)
                } else if silence_secs >= self.silence_duration_secs {
                    // Silence threshold reached but not enough speech accumulated —
                    // discard and reset to avoid stale state.
                    self.reset();
                    None
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
        self.total_voice_duration_secs = 0.0;
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
                                                       // Feed 1 second of speech at 16kHz (16000 samples) — satisfies 0.5s min_speech_duration.
        let speech = vec![0.5_f32; 16_000];
        assert!(det.feed(&speech, 16_000, 1).is_none());
        // Feed silence — should trigger because silence_duration_secs = 0.0 and we have 1s of speech.
        let silence = vec![0.001_f32; 16_000];
        let result = det.feed(&silence, 16_000, 1);
        assert!(result.is_some());
        let buf = result.expect("feed should return buffer after speech+silence");
        assert_eq!(buf.samples.len(), 16_000); // only speech samples
    }

    #[test]
    fn silence_detector_insufficient_speech_does_not_transcribe() {
        // 0.1 seconds of speech at 16kHz: 1600 samples < 0.5s minimum.
        let mut det = SilenceDetector::new(0.01, 0.0);
        let brief_sound = vec![0.5_f32; 1_600]; // 0.1s
        assert!(det.feed(&brief_sound, 16_000, 1).is_none());
        // Silence: min_speech_duration not met → must discard and return None.
        let silence = vec![0.001_f32; 16_000];
        assert!(det.feed(&silence, 16_000, 1).is_none());
        // State must be reset after discard.
        assert!(!det.speech_started);
    }

    #[test]
    fn silence_detector_reset_clears_state() {
        let mut det = SilenceDetector::new(0.01, 1.5);
        let speech = vec![0.5_f32; 16_000];
        det.feed(&speech, 16_000, 1);
        assert!(det.speech_started);
        det.reset();
        assert!(!det.speech_started);
        assert!(det.accumulated_samples.is_empty());
        assert!(det.last_voice_activity.is_none());
        assert_eq!(det.total_voice_duration_secs, 0.0);
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
