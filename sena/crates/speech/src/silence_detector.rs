//! Voice activity detection and silence gating for streaming STT.

use std::time::Instant;

/// Tracks voice activity and signals when an utterance should be finalized.
pub(crate) struct SilenceDetector {
    last_voice_activity: Option<Instant>,
    speech_started: bool,
    energy_threshold: f32,
    silence_duration_secs: f32,
    min_speech_duration_secs: f32,
    total_voice_duration_secs: f32,
}

impl SilenceDetector {
    pub(crate) fn new(energy_threshold: f32, silence_duration_secs: f32) -> Self {
        Self {
            last_voice_activity: None,
            speech_started: false,
            energy_threshold,
            silence_duration_secs,
            min_speech_duration_secs: 0.5,
            total_voice_duration_secs: 0.0,
        }
    }

    /// Feed a chunk and return true when speech followed by sufficient silence
    /// has been observed and the current utterance should be finalized.
    pub(crate) fn feed(&mut self, samples: &[f32], sample_rate: u32, channels: u16) -> bool {
        let rms = calculate_rms(samples);
        let is_voice = rms > self.energy_threshold;
        let now = Instant::now();
        let chunk_duration = samples.len() as f32 / (sample_rate as f32 * channels as f32);

        if is_voice {
            if !self.speech_started {
                self.speech_started = true;
                self.total_voice_duration_secs = 0.0;
            }

            self.total_voice_duration_secs += chunk_duration;
            self.last_voice_activity = Some(now);
            false
        } else if self.speech_started {
            if let Some(last_voice) = self.last_voice_activity {
                let silence_secs = now.duration_since(last_voice).as_secs_f32();
                if silence_secs >= self.silence_duration_secs {
                    let ready = self.total_voice_duration_secs >= self.min_speech_duration_secs;
                    self.reset();
                    ready
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    pub(crate) fn reset(&mut self) {
        self.speech_started = false;
        self.last_voice_activity = None;
        self.total_voice_duration_secs = 0.0;
    }
}

pub(crate) fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_sq: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_detector_no_speech_returns_false() {
        let mut detector = SilenceDetector::new(0.01, 1.5);
        let silence = vec![0.001; 160];
        assert!(!detector.feed(&silence, 16_000, 1));
    }

    #[test]
    fn silence_detector_speech_then_silence_returns_true() {
        let mut detector = SilenceDetector::new(0.01, 0.0);

        let speech = vec![0.5_f32; 16_000];
        assert!(!detector.feed(&speech, 16_000, 1));

        let silence = vec![0.001_f32; 16_000];
        assert!(detector.feed(&silence, 16_000, 1));
    }

    #[test]
    fn silence_detector_insufficient_speech_returns_false() {
        let mut detector = SilenceDetector::new(0.01, 0.0);
        let brief_sound = vec![0.5_f32; 1_600];
        assert!(!detector.feed(&brief_sound, 16_000, 1));

        let silence = vec![0.001_f32; 16_000];
        assert!(!detector.feed(&silence, 16_000, 1));
    }

    #[test]
    fn silence_detector_reset_clears_state() {
        let mut detector = SilenceDetector::new(0.01, 1.5);
        detector.feed(&vec![0.5_f32; 16_000], 16_000, 1);
        detector.reset();

        assert!(!detector.speech_started);
        assert!(detector.last_voice_activity.is_none());
        assert_eq!(detector.total_voice_duration_secs, 0.0);
    }
}
