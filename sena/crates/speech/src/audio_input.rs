//! Audio input (microphone capture) - minimal implementation for nested speech crate.
//!
//! Uses cpal for cross-platform microphone access. Simplified implementation
//! focused on feeding the SttBackend trait without voice activity detection or
//! complex resampling.

use crate::error::SttError;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

/// Audio chunk sent to STT actor for backend feeding.
#[derive(Debug, Clone, zeroize::ZeroizeOnDrop)]
pub struct AudioChunk {
    /// PCM samples (f32, mono).
    pub samples: Vec<f32>,
}

/// Audio input configuration.
#[derive(Debug, Clone)]
pub struct AudioInputConfig {
    /// Sample rate in Hz (16kHz is common for STT).
    pub sample_rate: u32,
    /// Buffer duration in seconds before emitting a chunk.
    pub buffer_duration_secs: f32,
}

impl Default for AudioInputConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            buffer_duration_secs: 1.0,
        }
    }
}

/// Audio input stream manager.
///
/// Internally owns a dedicated capture thread so the actor can remain Send.
pub struct AudioInputStream {
    #[allow(dead_code)]
    config: AudioInputConfig,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    capture_thread: Option<thread::JoinHandle<()>>,
}

impl AudioInputStream {
    /// Create and start a new audio input stream.
    ///
    /// Returns a receiver for audio chunks and the stream handle.
    pub fn start(
        config: AudioInputConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<AudioChunk>), SttError> {
        let (buffer_tx, buffer_rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), SttError>>();

        let worker_config = config.clone();
        let capture_thread = thread::spawn(move || {
            run_capture_loop(worker_config, buffer_tx, stop_rx, ready_tx);
        });

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                tracing::debug!("audio capture thread ready");
                Ok((
                    Self {
                        config,
                        stop_tx: Some(stop_tx),
                        capture_thread: Some(capture_thread),
                    },
                    buffer_rx,
                ))
            }
            Ok(Err(e)) => {
                tracing::error!("audio capture thread initialization failed: {}", e);
                let _ = stop_tx.send(());
                let _ = capture_thread.join();
                Err(e)
            }
            Err(_) => {
                tracing::error!("audio capture thread startup timed out");
                let _ = stop_tx.send(());
                let _ = capture_thread.join();
                Err(SttError::AudioCaptureFailed(
                    "audio capture startup timed out".to_string(),
                ))
            }
        }
    }

    /// Returns whether the capture thread is active.
    pub fn is_active(&self) -> bool {
        self.capture_thread.is_some()
    }
}

impl Drop for AudioInputStream {
    fn drop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
    }
}

fn run_capture_loop(
    config: AudioInputConfig,
    tx: mpsc::UnboundedSender<AudioChunk>,
    stop_rx: std::sync::mpsc::Receiver<()>,
    ready_tx: std::sync::mpsc::Sender<Result<(), SttError>>,
) {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            let _ = ready_tx.send(Err(SttError::AudioCaptureFailed(
                "no default input device".to_string(),
            )));
            return;
        }
    };

    let input_cfg = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(SttError::AudioCaptureFailed(format!(
                "get config failed: {}",
                e
            ))));
            return;
        }
    };

    let stream_config = input_cfg.config();
    let shared = Arc::new(Mutex::new(Vec::<f32>::new()));

    let stream = match build_stream_for_format(
        &device,
        &stream_config,
        input_cfg.sample_format(),
        Arc::clone(&shared),
        tx,
        config.sample_rate,
        config.buffer_duration_secs,
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = ready_tx.send(Err(SttError::AudioCaptureFailed(format!(
            "start stream failed: {}",
            e
        ))));
        return;
    }

    // Signal ready before entering the poll loop
    let _ = ready_tx.send(Ok(()));

    while stop_rx.try_recv().is_err() {
        thread::sleep(Duration::from_millis(50));
    }

    tracing::debug!("audio capture: stop signal received, exiting");
}

fn build_stream_for_format(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    shared: Arc<Mutex<Vec<f32>>>,
    tx: mpsc::UnboundedSender<AudioChunk>,
    target_rate: u32,
    buffer_duration_secs: f32,
) -> Result<cpal::Stream, SttError> {
    let err_fn = |err| {
        tracing::error!("audio input stream error: {}", err);
    };

    let input_rate = config.sample_rate.0;
    let channels = config.channels;

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| {
                process_input_chunk(
                    data,
                    channels,
                    input_rate,
                    target_rate,
                    buffer_duration_secs,
                    &shared,
                    &tx,
                );
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                let samples: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                process_input_chunk(
                    &samples,
                    channels,
                    input_rate,
                    target_rate,
                    buffer_duration_secs,
                    &shared,
                    &tx,
                );
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _| {
                let samples: Vec<f32> = data
                    .iter()
                    .map(|s| (*s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                    .collect();
                process_input_chunk(
                    &samples,
                    channels,
                    input_rate,
                    target_rate,
                    buffer_duration_secs,
                    &shared,
                    &tx,
                );
            },
            err_fn,
            None,
        ),
        _ => {
            return Err(SttError::AudioCaptureFailed(
                "unsupported microphone sample format".to_string(),
            ));
        }
    }
    .map_err(|e| SttError::AudioCaptureFailed(format!("build stream failed: {}", e)))?;

    Ok(stream)
}

#[allow(clippy::too_many_arguments)]
fn process_input_chunk(
    input: &[f32],
    channels: u16,
    input_rate: u32,
    target_rate: u32,
    buffer_duration_secs: f32,
    shared: &Arc<Mutex<Vec<f32>>>,
    tx: &mpsc::UnboundedSender<AudioChunk>,
) {
    let mono = downmix_to_mono(input, channels);
    if mono.is_empty() {
        return;
    }

    let input_buffer_len = (input_rate as f32 * buffer_duration_secs) as usize;
    if input_buffer_len == 0 {
        return;
    }

    let Ok(mut staging) = shared.lock() else {
        return;
    };
    staging.extend_from_slice(&mono);

    while staging.len() >= input_buffer_len {
        let chunk: Vec<f32> = staging.drain(..input_buffer_len).collect();
        let processed = if input_rate == target_rate {
            chunk
        } else {
            resample_linear(&chunk, input_rate, target_rate)
        };

        let _ = tx.send(AudioChunk { samples: processed });
    }
}

fn downmix_to_mono(input: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return input.to_vec();
    }

    let chan = channels as usize;
    input
        .chunks_exact(chan)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

fn resample_linear(input: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
    if input.is_empty() || input_rate == output_rate {
        return input.to_vec();
    }

    let ratio = output_rate as f64 / input_rate as f64;
    let output_len = (input.len() as f64 * ratio).round().max(1.0) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;

        let a = *input.get(idx).unwrap_or(&0.0);
        let b = *input.get(idx.saturating_add(1)).unwrap_or(&a);
        output.push(a + (b - a) * frac);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_input_config_defaults() {
        let config = AudioInputConfig::default();
        assert_eq!(config.sample_rate, 16_000);
        assert_eq!(config.buffer_duration_secs, 1.0);
    }

    #[test]
    fn downmix_mono_passthrough() {
        let mono = vec![0.1, 0.2, 0.3];
        let result = downmix_to_mono(&mono, 1);
        assert_eq!(result, mono);
    }

    #[test]
    fn downmix_stereo_to_mono() {
        let stereo = vec![0.0, 1.0, 0.5, 0.5];
        let result = downmix_to_mono(&stereo, 2);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], 0.5);
        assert_eq!(result[1], 0.5);
    }

    #[test]
    fn resample_passthrough_when_same_rate() {
        let input = vec![0.1, 0.2, 0.3];
        let output = resample_linear(&input, 16_000, 16_000);
        assert_eq!(output, input);
    }

    #[test]
    fn resample_upsampling() {
        let input = vec![0.0, 1.0];
        let output = resample_linear(&input, 8_000, 16_000);
        assert_eq!(output.len(), 4);
    }
}
