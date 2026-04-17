//! Backend trait definitions for STT and TTS.

use crate::error::{SttError, TtsError};
use crate::types::{AudioStream, SttEvent};

/// Audio device information.
#[derive(Debug, Clone)]
pub struct AudioDevice {
    /// Device name.
    pub name: String,
}

/// Speech-to-text backend trait.
///
/// Implementations must be Send to work with async actors.
pub trait SttBackend: Send {
    /// Returns the preferred number of PCM samples per chunk for this backend.
    fn preferred_chunk_samples(&self) -> usize;

    /// Feed PCM audio samples to the backend.
    ///
    /// Returns zero or more events emitted during processing.
    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SttError>;

    /// Flush any buffered audio and finalize transcription.
    ///
    /// Returns any remaining events.
    fn flush(&mut self) -> Result<Vec<SttEvent>, SttError>;

    /// List available audio input devices.
    fn list_audio_devices(&self) -> Result<Vec<AudioDevice>, SttError>;

    /// Backend name for logging and diagnostics.
    fn backend_name(&self) -> &'static str;
}

/// Text-to-speech backend trait.
///
/// Implementations must be Send to work with async actors.
pub trait TtsBackend: Send {
    /// Synthesize speech from text.
    ///
    /// Returns an audio stream ready for playback.
    fn synthesize(&mut self, text: &str) -> Result<AudioStream, TtsError>;

    /// Cancel any ongoing synthesis.
    fn cancel(&mut self);

    /// Flush audio output buffer.
    fn flush_buffer(&mut self);

    /// Backend name for logging and diagnostics.
    fn backend_name(&self) -> &'static str;
}
