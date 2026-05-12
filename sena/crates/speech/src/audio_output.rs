//! Audio output (speaker playback) - minimal implementation for nested speech crate.
//!
//! Uses cpal for cross-platform speaker access. Simplified implementation
//! focused on playing f32 PCM buffers from TTS backends with format adaptation.

use crate::error::TtsError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig, SupportedBufferSize};
use std::collections::VecDeque;
use std::convert::TryFrom;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Audio buffer to be played.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// PCM samples (f32, mono or stereo depending on channels).
    pub samples: Vec<f32>,
    /// Number of channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// Audio output configuration.
#[derive(Debug, Clone)]
pub struct AudioOutputConfig {
    /// Target sample rate in Hz (device native rate preferred).
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Buffer size per callback in frames.
    pub buffer_size_frames: usize,
}

impl Default for AudioOutputConfig {
    fn default() -> Self {
        Self {
            sample_rate: 22_050,
            channels: 1,
            buffer_size_frames: 1024,
        }
    }
}

/// Runtime audio format negotiated with the output device.
#[derive(Debug, Clone)]
pub struct AudioDeviceFormat {
    /// Sample rate accepted by the live output stream.
    pub sample_rate: u32,
    /// Channel count accepted by the live output stream.
    pub channels: u16,
    /// Sample format accepted by the live output stream.
    pub sample_format: SampleFormat,
    /// Requested fixed buffer size if one was selected, otherwise device default.
    pub buffer_size_frames: Option<u32>,
}

#[derive(Debug, Clone)]
struct NegotiatedOutputConfig {
    device_name: String,
    stream_config: StreamConfig,
    format: AudioDeviceFormat,
}

/// Audio output stream manager.
///
/// Internally owns a dedicated playback thread so the actor can remain Send.
pub struct AudioOutputStream {
    config: AudioOutputConfig,
    device_name: String,
    live_format: AudioDeviceFormat,
    play_tx: Option<mpsc::UnboundedSender<PlaybackCommand>>,
    playback_thread: Option<thread::JoinHandle<()>>,
}

enum PlaybackCommand {
    Enqueue {
        buffer: AudioBuffer,
        completion_tx: oneshot::Sender<Result<(), TtsError>>,
    },
    Clear,
    Stop,
}

struct PendingPlayback {
    samples: Vec<f32>,
    cursor: usize,
    completion_tx: Option<oneshot::Sender<Result<(), TtsError>>>,
}

impl PendingPlayback {
    fn new(samples: Vec<f32>, completion_tx: oneshot::Sender<Result<(), TtsError>>) -> Self {
        Self {
            samples,
            cursor: 0,
            completion_tx: Some(completion_tx),
        }
    }

    fn finish(mut self, result: Result<(), TtsError>) {
        if let Some(completion_tx) = self.completion_tx.take() {
            let _ = completion_tx.send(result);
        }
    }
}

#[derive(Default)]
struct PlaybackState {
    queue: VecDeque<PendingPlayback>,
}

impl AudioOutputStream {
    /// Create and start a new audio output stream.
    ///
    /// Returns the stream handle and a sender for audio buffers to play.
    pub fn start(config: AudioOutputConfig) -> Result<Self, TtsError> {
        let (play_tx, play_rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) =
            std::sync::mpsc::channel::<Result<NegotiatedOutputConfig, TtsError>>();

        let worker_config = config.clone();
        let playback_thread = thread::spawn(move || {
            run_playback_loop(worker_config, play_rx, ready_tx);
        });

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(negotiated)) => {
                tracing::debug!("audio playback thread ready");
                Ok(Self {
                    config,
                    device_name: negotiated.device_name,
                    live_format: negotiated.format,
                    play_tx: Some(play_tx),
                    playback_thread: Some(playback_thread),
                })
            }
            Ok(Err(e)) => {
                tracing::error!("audio playback thread initialization failed: {}", e);
                let _ = play_tx.send(PlaybackCommand::Stop);
                let _ = playback_thread.join();
                Err(e)
            }
            Err(_) => {
                tracing::error!("audio playback thread startup timed out");
                let _ = play_tx.send(PlaybackCommand::Stop);
                let _ = playback_thread.join();
                Err(TtsError::BackendError(
                    "audio playback startup timed out".to_string(),
                ))
            }
        }
    }

    /// Queue an audio buffer for playback and wait until the device callback drains it.
    pub async fn play_and_wait(&self, buffer: AudioBuffer) -> Result<(), TtsError> {
        let tx = self
            .play_tx
            .as_ref()
            .ok_or_else(|| TtsError::BackendError("audio output stream not active".to_string()))?;

        let (completion_tx, completion_rx) = oneshot::channel();
        tx.send(PlaybackCommand::Enqueue {
            buffer,
            completion_tx,
        })
        .map_err(|_| TtsError::BackendError("audio playback channel closed".to_string()))?;

        completion_rx.await.map_err(|_| {
            TtsError::BackendError("audio playback completion channel closed".to_string())
        })?
    }

    /// Clear any queued or in-flight audio samples.
    pub fn clear(&self) -> Result<(), TtsError> {
        let tx = self
            .play_tx
            .as_ref()
            .ok_or_else(|| TtsError::BackendError("audio output stream not active".to_string()))?;

        tx.send(PlaybackCommand::Clear)
            .map_err(|_| TtsError::BackendError("audio playback channel closed".to_string()))
    }

    /// Returns whether the playback thread is active.
    pub fn is_active(&self) -> bool {
        self.playback_thread.is_some()
    }

    /// Returns the device name associated with the live output stream.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Returns the runtime format negotiated with the live output stream.
    pub fn format(&self) -> AudioDeviceFormat {
        self.live_format.clone()
    }

    /// Returns the configured preference set used when opening the stream.
    pub fn preferred_config(&self) -> &AudioOutputConfig {
        &self.config
    }
}

impl Drop for AudioOutputStream {
    fn drop(&mut self) {
        if let Some(play_tx) = self.play_tx.take() {
            let _ = play_tx.send(PlaybackCommand::Stop);
        }
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
        }
    }
}

fn run_playback_loop(
    config: AudioOutputConfig,
    mut play_rx: mpsc::UnboundedReceiver<PlaybackCommand>,
    ready_tx: std::sync::mpsc::Sender<Result<NegotiatedOutputConfig, TtsError>>,
) {
    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(d) => d,
        None => {
            let _ = ready_tx.send(Err(TtsError::BackendError(
                "no audio output device available".to_string(),
            )));
            return;
        }
    };

    let negotiated = match negotiate_output_config(&device, &config) {
        Ok(negotiated) => negotiated,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    let playback_state = Arc::new(Mutex::new(PlaybackState::default()));

    let stream = match build_output_stream(
        &device,
        &negotiated,
        Arc::clone(&playback_state),
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = ready_tx.send(Err(TtsError::BackendError(format!(
            "stream play failed for device '{}' at {}Hz/{}ch {:?}: {}",
            negotiated.device_name,
            negotiated.format.sample_rate,
            negotiated.format.channels,
            negotiated.format.sample_format,
            e,
        ))));
        return;
    }

    tracing::info!(
        device = %negotiated.device_name,
        sample_rate = negotiated.format.sample_rate,
        channels = negotiated.format.channels,
        sample_format = ?negotiated.format.sample_format,
        buffer_size_frames = ?negotiated.format.buffer_size_frames,
        preferred_sample_rate = config.sample_rate,
        preferred_channels = config.channels,
        preferred_buffer_size_frames = config.buffer_size_frames,
        "audio output stream opened"
    );

    let _ = ready_tx.send(Ok(negotiated.clone()));

    while let Some(command) = play_rx.blocking_recv() {
        match command {
            PlaybackCommand::Enqueue {
                buffer,
                completion_tx,
            } => {
                let adapted = adapt_buffer_format(&buffer, &negotiated.format);
                let sample_count = adapted.samples.len();
                let mut state = playback_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state
                    .queue
                    .push_back(PendingPlayback::new(adapted.samples, completion_tx));
                tracing::trace!(
                    device = %negotiated.device_name,
                    queued_samples = sample_count,
                    input_sample_rate = buffer.sample_rate,
                    input_channels = buffer.channels,
                    output_sample_rate = negotiated.format.sample_rate,
                    output_channels = negotiated.format.channels,
                    "queued audio buffer for playback"
                );
            }
            PlaybackCommand::Clear => {
                clear_playback_state(&playback_state, "playback interrupted");
            }
            PlaybackCommand::Stop => {
                tracing::debug!(device = %negotiated.device_name, "audio playback loop received stop signal");
                break;
            }
        }
    }

    if play_rx.is_closed() {
        tracing::debug!(device = %negotiated.device_name, "audio playback channel disconnected");
    }

    clear_playback_state(&playback_state, "audio output stopped");
    drop(stream);
    tracing::debug!(device = %negotiated.device_name, "audio playback loop exiting");
}

fn negotiate_output_config(
    device: &cpal::Device,
    preferred: &AudioOutputConfig,
) -> Result<NegotiatedOutputConfig, TtsError> {
    let device_name = device
        .name()
        .unwrap_or_else(|_| "<unknown output device>".to_string());

    let supported_config = device.default_output_config().map_err(|e| {
        TtsError::BackendError(format!(
            "default output config unavailable for device '{}': {}",
            device_name, e
        ))
    })?;

    let mut stream_config = supported_config.config();
    if let Some(buffer_size_frames) = select_buffer_size_frames(
        supported_config.buffer_size(),
        preferred.buffer_size_frames,
    ) {
        stream_config.buffer_size = cpal::BufferSize::Fixed(buffer_size_frames);
    }

    let format = AudioDeviceFormat {
        sample_rate: stream_config.sample_rate.0,
        channels: stream_config.channels,
        sample_format: supported_config.sample_format(),
        buffer_size_frames: match stream_config.buffer_size {
            cpal::BufferSize::Default => None,
            cpal::BufferSize::Fixed(frames) => Some(frames),
        },
    };

    Ok(NegotiatedOutputConfig {
        device_name,
        stream_config,
        format,
    })
}

fn select_buffer_size_frames(
    supported_buffer_size: &SupportedBufferSize,
    preferred_frames: usize,
) -> Option<u32> {
    let preferred_frames = u32::try_from(preferred_frames).ok()?;

    match supported_buffer_size {
        SupportedBufferSize::Range { min, max }
            if preferred_frames >= *min && preferred_frames <= *max =>
        {
            Some(preferred_frames)
        }
        _ => None,
    }
}

fn build_output_stream(
    device: &cpal::Device,
    negotiated: &NegotiatedOutputConfig,
    playback_state: Arc<Mutex<PlaybackState>>,
) -> Result<cpal::Stream, TtsError> {
    let channels = negotiated.format.channels as usize;
    let device_name = negotiated.device_name.clone();
    let format = negotiated.format.clone();
    let error_state = Arc::clone(&playback_state);

    let err_fn = move |err| {
        tracing::error!(
            device = %device_name,
            sample_rate = format.sample_rate,
            channels = format.channels,
            sample_format = ?format.sample_format,
            error = %err,
            "audio output stream error"
        );
        clear_playback_state(&error_state, "audio output stream failed");
    };

    let stream = match negotiated.format.sample_format {
        SampleFormat::F32 => device.build_output_stream(
            &negotiated.stream_config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                write_output_data(data, &playback_state, channels);
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            &negotiated.stream_config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                let mut state = playback_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for sample in data.iter_mut() {
                    if let Some(source_sample) = next_output_sample(&mut state) {
                        *sample = (source_sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    } else {
                        *sample = 0;
                    }
                }
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            &negotiated.stream_config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                let mut state = playback_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for sample in data.iter_mut() {
                    if let Some(source_sample) = next_output_sample(&mut state) {
                        let source_sample = source_sample.clamp(-1.0, 1.0);
                        *sample = ((source_sample + 1.0) * 0.5 * u16::MAX as f32) as u16;
                    } else {
                        *sample = u16::MAX / 2;
                    }
                }
            },
            err_fn,
            None,
        ),
        _ => {
            return Err(TtsError::BackendError(format!(
                "unsupported sample format for device '{}' at {}Hz/{}ch: {:?}",
                negotiated.device_name,
                negotiated.format.sample_rate,
                negotiated.format.channels,
                negotiated.format.sample_format,
            )));
        }
    };

    stream.map_err(|e| {
        TtsError::BackendError(format!(
            "build output stream failed for device '{}' at {}Hz/{}ch {:?}: {}",
            negotiated.device_name,
            negotiated.format.sample_rate,
            negotiated.format.channels,
            negotiated.format.sample_format,
            e,
        ))
    })
}

fn next_output_sample(state: &mut PlaybackState) -> Option<f32> {
    loop {
        let playback = state.queue.front_mut()?;
        if playback.cursor < playback.samples.len() {
            let sample = playback.samples[playback.cursor];
            playback.cursor += 1;
            let finished = playback.cursor >= playback.samples.len();
            if finished && let Some(playback) = state.queue.pop_front() {
                playback.finish(Ok(()));
            }
            return Some(sample);
        }

        if let Some(playback) = state.queue.pop_front() {
            playback.finish(Ok(()));
        }
    }
}

fn clear_playback_state(state: &Arc<Mutex<PlaybackState>>, reason: &str) -> usize {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut cleared = 0;

    while let Some(playback) = state.queue.pop_front() {
        cleared += 1;
        playback.finish(Err(TtsError::BackendError(reason.to_string())));
    }

    cleared
}

fn write_output_data(data: &mut [f32], state: &Arc<Mutex<PlaybackState>>, channels: usize) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for frame in data.chunks_mut(channels) {
        let source_sample = next_output_sample(&mut state).unwrap_or(0.0).clamp(-1.0, 1.0);
        for sample in frame.iter_mut() {
            *sample = source_sample;
        }
    }
}

/// Adapt audio buffer to target format (sample rate and channel count).
fn adapt_buffer_format(source: &AudioBuffer, target_format: &AudioDeviceFormat) -> AudioBuffer {
    let resampled = if source.sample_rate == target_format.sample_rate {
        source.samples.clone()
    } else {
        resample_linear(
            &source.samples,
            source.sample_rate,
            target_format.sample_rate,
            source.channels as usize,
        )
    };

    let samples_final = adapt_channels(
        &resampled,
        source.channels as usize,
        target_format.channels as usize,
    );

    AudioBuffer {
        samples: samples_final,
        channels: target_format.channels,
        sample_rate: target_format.sample_rate,
    }
}

fn adapt_channels(samples: &[f32], source_channels: usize, target_channels: usize) -> Vec<f32> {
    if samples.is_empty() || source_channels == 0 || target_channels == 0 {
        return Vec::new();
    }

    if source_channels == target_channels {
        return samples.to_vec();
    }

    if source_channels == 1 {
        return samples
            .iter()
            .flat_map(|&sample| std::iter::repeat_n(sample, target_channels))
            .collect();
    }

    if target_channels == 1 {
        return samples
            .chunks(source_channels)
            .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
            .collect();
    }

    let frame_count = samples.len().div_ceil(source_channels);
    let mut adapted = Vec::with_capacity(frame_count * target_channels);

    for frame in samples.chunks(source_channels) {
        for channel in 0..target_channels {
            let sample = frame
                .get(channel)
                .copied()
                .or_else(|| frame.first().copied())
                .unwrap_or(0.0);
            adapted.push(sample);
        }
    }

    adapted
}

/// Simple linear interpolation resampler.
fn resample_linear(samples: &[f32], src_rate: u32, dst_rate: u32, channels: usize) -> Vec<f32> {
    if samples.is_empty() || channels == 0 {
        return Vec::new();
    }

    if src_rate == dst_rate {
        return samples.to_vec();
    }

    let input_frames = samples.len().div_ceil(channels);
    if input_frames == 0 {
        return Vec::new();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let output_frames = (input_frames as f64 / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_frames * channels);

    for frame_index in 0..output_frames {
        let src_frame = frame_index as f64 * ratio;
        let idx0 = src_frame.floor() as usize;
        let idx1 = (idx0 + 1).min(input_frames - 1);
        let frac = src_frame - idx0 as f64;

        for channel in 0..channels {
            let sample0 = frame_sample(samples, idx0, channel, channels);
            let sample1 = frame_sample(samples, idx1, channel, channels);
            let sample = sample0 * (1.0 - frac) as f32 + sample1 * frac as f32;
            output.push(sample);
        }
    }

    output
}

fn frame_sample(samples: &[f32], frame_index: usize, channel_index: usize, channels: usize) -> f32 {
    let sample_index = frame_index * channels + channel_index;
    samples
        .get(sample_index)
        .copied()
        .or_else(|| samples.get(frame_index * channels).copied())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn test_format(sample_rate: u32, channels: u16) -> AudioDeviceFormat {
        AudioDeviceFormat {
            sample_rate,
            channels,
            sample_format: SampleFormat::F32,
            buffer_size_frames: None,
        }
    }

    #[test]
    fn audio_buffer_channel_mono_to_stereo() {
        let source = AudioBuffer {
            samples: vec![0.1, 0.2, 0.3],
            channels: 1,
            sample_rate: 16000,
        };
        let format = test_format(16000, 2);

        let adapted = adapt_buffer_format(&source, &format);
        assert_eq!(adapted.channels, 2);
        assert_eq!(adapted.samples.len(), 6);
        assert_eq!(adapted.samples, vec![0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
    }

    #[test]
    fn audio_buffer_channel_stereo_to_mono() {
        let source = AudioBuffer {
            samples: vec![0.1, 0.2, 0.3, 0.4],
            channels: 2,
            sample_rate: 16000,
        };
        let format = test_format(16000, 1);

        let adapted = adapt_buffer_format(&source, &format);
        assert_eq!(adapted.channels, 1);
        assert_eq!(adapted.samples.len(), 2);
        assert!((adapted.samples[0] - 0.15).abs() < 0.001);
        assert!((adapted.samples[1] - 0.35).abs() < 0.001);
    }

    #[test]
    fn audio_buffer_channel_mono_to_multichannel() {
        let source = AudioBuffer {
            samples: vec![0.1, 0.2],
            channels: 1,
            sample_rate: 22050,
        };
        let format = test_format(22050, 4);

        let adapted = adapt_buffer_format(&source, &format);
        assert_eq!(adapted.channels, 4);
        assert_eq!(adapted.samples, vec![0.1, 0.1, 0.1, 0.1, 0.2, 0.2, 0.2, 0.2]);
    }

    #[test]
    fn audio_buffer_resample_upsampling() {
        let samples = vec![0.0, 1.0, 0.0];
        let resampled = resample_linear(&samples, 8000, 16000, 1);
        assert!(resampled.len() > samples.len());
        assert!(resampled.len() <= samples.len() * 2 + 1);
    }

    #[test]
    fn audio_buffer_resample_downsampling() {
        let samples = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        let resampled = resample_linear(&samples, 16000, 8000, 1);
        assert!(resampled.len() < samples.len());
        assert!(resampled.len() >= samples.len() / 2);
    }

    #[test]
    fn audio_buffer_resample_preserves_stereo_channels() {
        let samples = vec![0.0, 1.0, 1.0, 0.0];
        let resampled = resample_linear(&samples, 2, 3, 2);

        assert_eq!(resampled.len(), 6);
        assert!((resampled[0] - 0.0).abs() < 0.001);
        assert!((resampled[1] - 1.0).abs() < 0.001);
        assert!((resampled[2] - 0.6666667).abs() < 0.001);
        assert!((resampled[3] - 0.33333334).abs() < 0.001);
        assert!((resampled[4] - 1.0).abs() < 0.001);
        assert!((resampled[5] - 0.0).abs() < 0.001);
    }

    #[test]
    fn audio_buffer_no_adaptation_needed() {
        let source = AudioBuffer {
            samples: vec![0.1, 0.2, 0.3],
            channels: 1,
            sample_rate: 16000,
        };
        let format = test_format(16000, 1);

        let adapted = adapt_buffer_format(&source, &format);
        assert_eq!(adapted.samples, source.samples);
    }

    #[tokio::test]
    async fn write_output_data_signals_completion_when_buffer_drains() {
        let state = Arc::new(Mutex::new(PlaybackState::default()));
        let (completion_tx, completion_rx) = oneshot::channel();

        {
            let mut guard = state.lock().expect("state mutex should not be poisoned");
            guard
                .queue
                .push_back(PendingPlayback::new(vec![0.1, 0.2], completion_tx));
        }

        let mut output = vec![0.0; 2];
        write_output_data(&mut output, &state, 1);

        assert_eq!(output, vec![0.1, 0.2]);
        completion_rx
            .await
            .expect("completion should be sent")
            .expect("playback should complete successfully");
        assert!(state.lock().expect("state mutex").queue.is_empty());
    }

    #[tokio::test]
    async fn clear_playback_state_interrupts_pending_audio() {
        let state = Arc::new(Mutex::new(PlaybackState::default()));
        let (completion_tx, completion_rx) = oneshot::channel();

        {
            let mut guard = state.lock().expect("state mutex should not be poisoned");
            guard
                .queue
                .push_back(PendingPlayback::new(vec![0.1, 0.2], completion_tx));
        }

        let cleared = clear_playback_state(&state, "interrupted");
        assert_eq!(cleared, 1);
        assert!(state.lock().expect("state mutex").queue.is_empty());

        let result = completion_rx.await.expect("clear should notify waiter");
        assert!(matches!(result, Err(TtsError::BackendError(_))));
    }
}
