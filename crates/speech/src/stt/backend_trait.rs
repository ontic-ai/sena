//! STT backend trait and associated event types.

use crate::SpeechError;

/// Events produced by an STT backend in response to audio input.
///
/// Backends return these from `feed()` and `flush()`. The actor maps them
/// to bus `SpeechEvent` variants, adding request IDs and routing context.
#[derive(Debug, Clone)]
pub enum SttEvent {
    /// Non-final partial transcription (streaming / listen mode).
    Partial { text: String, confidence: f32 },
    /// Final transcription for this utterance.
    Completed { text: String, confidence: f32 },
}

/// Trait that every STT backend must implement.
///
/// Backends are responsible for:
/// - Managing internal worker threads and channel state.
/// - Managing per-session rolling buffers and accumulation counters.
/// - Returning typed `SttEvent` values per audio chunk.
///
/// The actor owns a `Box<dyn SttBackend>` wrapped in `Arc<Mutex<>>` so that
/// blocking `feed()` / `flush()` calls can be dispatched via `spawn_blocking`.
pub trait SttBackend: Send + 'static {
    /// Preferred number of PCM samples per `feed()` call.
    ///
    /// - Parakeet: 2560 (160 ms at 16 kHz — required chunk size)
    /// - Sherpa:   3200 (200 ms at 16 kHz — decode interval)
    /// - Whisper: 16000 (1 s at 16 kHz — fake-streaming window)
    /// - Mock:    16000
    ///
    /// The actor uses this value to configure `AudioInputStream::buffer_duration_secs`
    /// for listen-mode sessions.
    fn preferred_chunk_samples(&self) -> usize;

    /// Process an audio chunk.
    ///
    /// Implementations may accumulate samples internally and return events only
    /// when enough audio has been collected or a decode interval has elapsed.
    /// Returns an empty `Vec` when no events are ready yet.
    ///
    /// # Errors
    /// Returns `SpeechError` if the worker channel is closed or a decode error occurs.
    fn feed(&mut self, pcm: &[f32]) -> Result<Vec<SttEvent>, SpeechError>;

    /// Flush buffered state and return any pending final transcription.
    ///
    /// Called:
    /// - When VAD detects end-of-speech (listen mode final).
    /// - After `feed()` in always-on mode to force output.
    /// - At session stop to clean up internal state.
    ///
    /// Implementations must reset their internal rolling buffers and timers
    /// so the next session begins with clean state.
    ///
    /// # Errors
    /// Returns `SpeechError` if the worker channel is closed or a decode error occurs.
    fn flush(&mut self) -> Result<Vec<SttEvent>, SpeechError>;

    /// Backend name for telemetry and log messages.
    fn backend_name(&self) -> &'static str;

    /// Estimated VRAM usage in MB (for `SttTelemetryUpdate` events).
    fn vram_mb(&self) -> u64;
}
