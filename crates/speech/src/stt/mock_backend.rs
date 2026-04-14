//! Mock STT backend — used in tests; no model loading or worker threads.

use crate::silence_detector::calculate_rms;
use crate::SpeechError;
use super::backend_trait::{SttBackend, SttEvent};

/// No-op STT backend for unit tests.
///
/// `feed()` returns a `Completed` event immediately based on audio energy.
/// `flush()` is a no-op that returns an empty event list.
pub struct MockSttBackend;

impl SttBackend for MockSttBackend {
    fn preferred_chunk_samples(&self) -> usize {
        16_000
    }

    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SpeechError> {
        let rms = calculate_rms(pcm);
        if rms < 0.001 {
            Ok(vec![SttEvent::Completed {
                text: String::new(),
                confidence: 0.1,
            }])
        } else {
            Ok(vec![SttEvent::Completed {
                text: "mock transcription".to_string(),
                confidence: 0.85,
            }])
        }
    }

    fn flush(&mut self) -> Result<Vec<SttEvent>, SpeechError> {
        Ok(vec![])
    }

    fn backend_name(&self) -> &'static str {
        "mock"
    }

    fn vram_mb(&self) -> u64 {
        0
    }
}
