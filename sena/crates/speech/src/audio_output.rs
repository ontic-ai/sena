//! Audio output (speaker playback) - minimal implementation for nested speech crate.
//!
//! Uses cpal for cross-platform speaker access. Simplified implementation
//! focused on playing f32 PCM buffers from TTS backends with format adaptation.

use crate::error::TtsError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig, SupportedBufferSize};
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction, calculate_cutoff,
};
use std::collections::VecDeque;
use std::convert::TryFrom;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const RUBATO_SINC_LEN: usize = 256;
const RUBATO_OVERSAMPLING_FACTOR: usize = 256;
const RUBATO_INPUT_CHUNK_FRAMES: usize = 1024;

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

struct PlaybackFormatAdapter {
    source_sample_rate: u32,
    source_channels: usize,
    target_format: AudioDeviceFormat,
    resampler: Option<SincFormatResampler>,
}

impl PlaybackFormatAdapter {
    fn new(
        source_sample_rate: u32,
        source_channels: u16,
        target_format: AudioDeviceFormat,
    ) -> Result<Self, TtsError> {
        let source_channels = usize::from(source_channels);
        let resampler = if source_sample_rate == target_format.sample_rate {
            None
        } else {
            Some(SincFormatResampler::new(
                source_sample_rate,
                target_format.sample_rate,
                source_channels,
            )?)
        };

        Ok(Self {
            source_sample_rate,
            source_channels,
            target_format,
            resampler,
        })
    }

    fn adapt_buffer(&mut self, source: &AudioBuffer) -> Result<AudioBuffer, TtsError> {
        if source.sample_rate != self.source_sample_rate {
            return Err(TtsError::BackendError(format!(
                "unexpected audio sample rate: expected {}Hz, got {}Hz",
                self.source_sample_rate, source.sample_rate,
            )));
        }

        if usize::from(source.channels) != self.source_channels {
            return Err(TtsError::BackendError(format!(
                "unexpected audio channel count: expected {}, got {}",
                self.source_channels, source.channels,
            )));
        }

        let resampled = if let Some(resampler) = &mut self.resampler {
            resampler.process_clip(&source.samples)?
        } else {
            source.samples.clone()
        };

        let samples = adapt_channels(
            &resampled,
            self.source_channels,
            usize::from(self.target_format.channels),
        );

        Ok(AudioBuffer {
            samples,
            channels: self.target_format.channels,
            sample_rate: self.target_format.sample_rate,
        })
    }

    fn reset(&mut self) {
        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
    }

    fn uses_resampler(&self) -> bool {
        self.resampler.is_some()
    }
}

struct SincFormatResampler {
    inner: Async<f32>,
    channels: usize,
    delay_frames_to_trim: usize,
}

impl SincFormatResampler {
    fn new(src_rate: u32, dst_rate: u32, channels: usize) -> Result<Self, TtsError> {
        if channels == 0 {
            return Err(TtsError::BackendError(
                "rubato sinc resampler requires at least one channel".to_string(),
            ));
        }

        let window = WindowFunction::BlackmanHarris2;
        let inner = Async::<f32>::new_sinc(
            dst_rate as f64 / src_rate as f64,
            1.0,
            &SincInterpolationParameters {
                sinc_len: RUBATO_SINC_LEN,
                f_cutoff: calculate_cutoff(RUBATO_SINC_LEN, window),
                oversampling_factor: RUBATO_OVERSAMPLING_FACTOR,
                interpolation: SincInterpolationType::Cubic,
                window,
            },
            RUBATO_INPUT_CHUNK_FRAMES,
            channels,
            FixedAsync::Input,
        )
        .map_err(|e| {
            TtsError::BackendError(format!(
                "rubato sinc resampler init failed for {}Hz -> {}Hz: {}",
                src_rate, dst_rate, e,
            ))
        })?;

        let delay_frames_to_trim = inner.output_delay();

        Ok(Self {
            inner,
            channels,
            delay_frames_to_trim,
        })
    }

    fn process_clip(&mut self, interleaved_samples: &[f32]) -> Result<Vec<f32>, TtsError> {
        if interleaved_samples.is_empty() {
            return Ok(Vec::new());
        }

        if !interleaved_samples.len().is_multiple_of(self.channels) {
            return Err(TtsError::BackendError(format!(
                "interleaved sample count {} is not divisible by channel count {}",
                interleaved_samples.len(), self.channels,
            )));
        }

        let input_frames = interleaved_samples.len() / self.channels;
        if input_frames == 0 {
            return Ok(Vec::new());
        }

        let expected_output_frames =
            (input_frames as f64 * self.inner.resample_ratio()).ceil() as usize;
        let mut output = Vec::with_capacity(expected_output_frames * self.channels);
        let mut frame_offset = 0;

        while input_frames.saturating_sub(frame_offset) >= self.inner.input_frames_next() {
            let chunk_frames = self.inner.input_frames_next();
            let input_chunk = build_planar_input(
                interleaved_samples,
                self.channels,
                frame_offset,
                chunk_frames,
                chunk_frames,
            );
            self.process_chunk(input_chunk, None, &mut output)?;
            frame_offset += chunk_frames;
        }

        let remaining_frames = input_frames.saturating_sub(frame_offset);
        if remaining_frames > 0 {
            let chunk_frames = self.inner.input_frames_next();
            let input_chunk = build_planar_input(
                interleaved_samples,
                self.channels,
                frame_offset,
                remaining_frames,
                chunk_frames,
            );
            self.process_chunk(input_chunk, Some(remaining_frames), &mut output)?;
        }

        let target_samples = expected_output_frames * self.channels;
        let max_flush_pumps = 8;
        let mut flush_pumps = 0;
        while output.len() < target_samples {
            if flush_pumps >= max_flush_pumps {
                return Err(TtsError::BackendError(format!(
                    "rubato sinc resampler flush stalled after {} zero-padded chunks",
                    max_flush_pumps,
                )));
            }

            let chunk_frames = self.inner.input_frames_next();
            let zero_chunk = vec![vec![0.0; chunk_frames]; self.channels];
            self.process_chunk(zero_chunk, Some(0), &mut output)?;
            flush_pumps += 1;
        }

        output.truncate(target_samples);
        Ok(output)
    }

    fn reset(&mut self) {
        self.inner.reset();
        self.delay_frames_to_trim = self.inner.output_delay();
    }

    fn process_chunk(
        &mut self,
        input_chunk: Vec<Vec<f32>>,
        partial_len: Option<usize>,
        output: &mut Vec<f32>,
    ) -> Result<(), TtsError> {
        let input_frames = input_chunk.first().map_or(0, Vec::len);
        let input = SequentialSliceOfVecs::new(&input_chunk, self.channels, input_frames)
            .map_err(|e| {
                TtsError::BackendError(format!(
                    "rubato input adapter init failed for {} frames: {}",
                    input_frames, e,
                ))
            })?;

        let output_frames = self.inner.output_frames_max();
        let mut output_chunk = vec![vec![0.0; output_frames]; self.channels];
        let mut output_adapter =
            SequentialSliceOfVecs::new_mut(&mut output_chunk, self.channels, output_frames)
                .map_err(|e| {
                    TtsError::BackendError(format!(
                        "rubato output adapter init failed for {} frames: {}",
                        output_frames, e,
                    ))
                })?;

        let indexing = partial_len.map(|partial_len| Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: Some(partial_len),
            active_channels_mask: None,
        });

        let (_consumed, produced_frames) = self
            .inner
            .process_into_buffer(&input, &mut output_adapter, indexing.as_ref())
            .map_err(|e| {
                TtsError::BackendError(format!(
                    "rubato sinc resample failed for {} input frames: {}",
                    input_frames, e,
                ))
            })?;

        let frames_to_skip = self.delay_frames_to_trim.min(produced_frames);
        self.delay_frames_to_trim -= frames_to_skip;

        if produced_frames <= frames_to_skip {
            return Ok(());
        }

        output.reserve((produced_frames - frames_to_skip) * self.channels);
        for frame_samples in output_chunk
            .first()
            .into_iter()
            .flat_map(|channel| channel.iter().enumerate())
            .skip(frames_to_skip)
            .take(produced_frames - frames_to_skip)
        {
            let (frame_index, _) = frame_samples;
            for channel_samples in output_chunk.iter().take(self.channels) {
                output.push(channel_samples[frame_index]);
            }
        }

        Ok(())
    }
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
    let mut format_adapter = match PlaybackFormatAdapter::new(
        config.sample_rate,
        config.channels,
        negotiated.format.clone(),
    ) {
        Ok(adapter) => adapter,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

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
        resampler = if format_adapter.uses_resampler() { "rubato sinc" } else { "none" },
        "audio output stream opened"
    );

    let _ = ready_tx.send(Ok(negotiated.clone()));

    while let Some(command) = play_rx.blocking_recv() {
        match command {
            PlaybackCommand::Enqueue {
                buffer,
                completion_tx,
            } => {
                match format_adapter.adapt_buffer(&buffer) {
                    Ok(adapted) => {
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
                    Err(error) => {
                        tracing::error!(
                            device = %negotiated.device_name,
                            input_sample_rate = buffer.sample_rate,
                            input_channels = buffer.channels,
                            error = %error,
                            "failed to adapt audio buffer for playback"
                        );
                        let _ = completion_tx.send(Err(error));
                    }
                }
            }
            PlaybackCommand::Clear => {
                clear_playback_state(&playback_state, "playback interrupted");
                format_adapter.reset();
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
    format_adapter.reset();
    drop(stream);
    tracing::debug!(device = %negotiated.device_name, "audio playback loop exiting");
}

fn build_planar_input(
    interleaved_samples: &[f32],
    channels: usize,
    frame_offset: usize,
    available_frames: usize,
    chunk_frames: usize,
) -> Vec<Vec<f32>> {
    let mut planar = vec![vec![0.0; chunk_frames]; channels];

    for (channel_index, channel_samples) in planar.iter_mut().enumerate().take(channels) {
        for (frame_index, sample) in channel_samples.iter_mut().enumerate().take(available_frames) {
            let input_frame_index = frame_offset + frame_index;
            let input_offset = input_frame_index * channels;
            *sample = interleaved_samples[input_offset + channel_index];
        }
    }

    planar
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn test_adapter(
        source_sample_rate: u32,
        source_channels: u16,
        target_sample_rate: u32,
        target_channels: u16,
    ) -> PlaybackFormatAdapter {
        PlaybackFormatAdapter::new(
            source_sample_rate,
            source_channels,
            test_format(target_sample_rate, target_channels),
        )
        .expect("playback format adapter should initialize")
    }

    fn assert_samples_close(actual: &[f32], expected: &[f32], epsilon: f32) {
        assert_eq!(actual.len(), expected.len(), "sample lengths should match");
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual - expected).abs() <= epsilon,
                "sample {index} differed: actual={actual}, expected={expected}, epsilon={epsilon}",
            );
        }
    }

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

        let mut adapter = test_adapter(16000, 1, 16000, 2);
        let adapted = adapter
            .adapt_buffer(&source)
            .expect("mono -> stereo adaptation should succeed");
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

        let mut adapter = test_adapter(16000, 2, 16000, 1);
        let adapted = adapter
            .adapt_buffer(&source)
            .expect("stereo -> mono adaptation should succeed");
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

        let mut adapter = test_adapter(22050, 1, 22050, 4);
        let adapted = adapter
            .adapt_buffer(&source)
            .expect("mono -> multichannel adaptation should succeed");
        assert_eq!(adapted.channels, 4);
        assert_eq!(adapted.samples, vec![0.1, 0.1, 0.1, 0.1, 0.2, 0.2, 0.2, 0.2]);
    }

    #[test]
    fn audio_buffer_resample_upsampling() {
        let source = AudioBuffer {
            samples: vec![0.0, 1.0, 0.0, -1.0],
            channels: 1,
            sample_rate: 8000,
        };

        let mut adapter = test_adapter(8000, 1, 16000, 1);
        let adapted = adapter
            .adapt_buffer(&source)
            .expect("upsampling should succeed");

        assert!(adapted.samples.len() > source.samples.len());
    }

    #[test]
    fn audio_buffer_resample_downsampling() {
        let source = AudioBuffer {
            samples: vec![0.0, 0.5, 1.0, 0.5, 0.0, -0.5],
            channels: 1,
            sample_rate: 16000,
        };

        let mut adapter = test_adapter(16000, 1, 8000, 1);
        let adapted = adapter
            .adapt_buffer(&source)
            .expect("downsampling should succeed");

        assert!(adapted.samples.len() < source.samples.len());
    }

    #[test]
    fn audio_buffer_resample_preserves_stereo_channels() {
        let frames = 512;
        let source = AudioBuffer {
            samples: (0..frames).flat_map(|_| [0.0, 1.0]).collect(),
            channels: 2,
            sample_rate: 32000,
        };

        let mut adapter = test_adapter(32000, 2, 48000, 2);
        let adapted = adapter
            .adapt_buffer(&source)
            .expect("stereo resampling should succeed");

        assert_eq!(adapted.channels, 2);
        assert!(adapted.samples.len() > source.samples.len());
        let adapted_frames: Vec<_> = adapted.samples.chunks_exact(2).collect();
        let center_frames = &adapted_frames[64..adapted_frames.len() - 64];
        let left_avg = center_frames
            .iter()
            .map(|frame| frame[0].abs())
            .sum::<f32>()
            / center_frames.len() as f32;
        let right_avg = center_frames.iter().map(|frame| frame[1]).sum::<f32>()
            / center_frames.len() as f32;

        assert!(left_avg < 0.02);
        assert!((right_avg - 1.0).abs() < 0.02);
    }

    #[test]
    fn audio_buffer_no_adaptation_needed() {
        let source = AudioBuffer {
            samples: vec![0.1, 0.2, 0.3],
            channels: 1,
            sample_rate: 16000,
        };

        let mut adapter = test_adapter(16000, 1, 16000, 1);
        let adapted = adapter
            .adapt_buffer(&source)
            .expect("identity adaptation should succeed");
        assert_eq!(adapted.samples, source.samples);
    }

    #[test]
    fn sinc_resampler_reset_restores_initial_output() {
        let source = AudioBuffer {
            samples: (0..256).map(|index| index as f32 / 256.0).collect(),
            channels: 1,
            sample_rate: 22050,
        };

        let mut adapter = test_adapter(22050, 1, 96000, 1);
        let first = adapter
            .adapt_buffer(&source)
            .expect("first resample should succeed");

        adapter.reset();

        let after_reset = adapter
            .adapt_buffer(&source)
            .expect("resample after reset should succeed");

        assert_samples_close(&first.samples, &after_reset.samples, 0.0001);
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
