//! Audio input (microphone capture) - cross-platform.
//!
//! Uses cpal for cross-platform microphone access on Windows (WASAPI),
//! macOS (CoreAudio), and Linux (ALSA).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use tokio::sync::mpsc;

use crate::{AudioBuffer, SpeechError};

/// Audio input configuration.
#[derive(Debug, Clone)]
pub struct AudioInputConfig {
    /// Sample rate in Hz (Whisper requires 16kHz).
    pub sample_rate: u32,
    /// Buffer duration in seconds.
    pub buffer_duration_secs: f32,
    /// Energy threshold for voice activity detection (RMS > threshold).
    pub energy_threshold: f32,
}

impl Default for AudioInputConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            buffer_duration_secs: 3.0,
            energy_threshold: 0.01,
        }
    }
}

/// Audio input stream manager.
///
/// Internally owns a dedicated capture thread so the actor can remain Send.
pub struct AudioInputStream {
    config: AudioInputConfig,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    capture_thread: Option<thread::JoinHandle<()>>,
}

impl AudioInputStream {
    /// Create and start a new audio input stream.
    ///
    /// Returns a receiver for voice-active audio buffers and the stream handle.
    pub fn start(
        config: AudioInputConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<AudioBuffer>), SpeechError> {
        // Preflight device checks so startup errors are surfaced synchronously.
        let host = cpal::default_host();
        let _device = host
            .default_input_device()
            .ok_or_else(|| SpeechError::AudioCaptureFailed("no input device found".to_string()))?;

        let (buffer_tx, buffer_rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();

        let worker_config = config.clone();
        let capture_thread = thread::spawn(move || {
            let ready = run_capture_loop(worker_config, buffer_tx, stop_rx);
            let _ = ready_tx.send(ready);
        });

        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok((
                Self {
                    config,
                    stop_tx: Some(stop_tx),
                    capture_thread: Some(capture_thread),
                },
                buffer_rx,
            )),
            Ok(Err(e)) => {
                let _ = stop_tx.send(());
                let _ = capture_thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = stop_tx.send(());
                let _ = capture_thread.join();
                Err(SpeechError::AudioCaptureFailed(
                    "audio capture startup timed out".to_string(),
                ))
            }
        }
    }

    /// Get the current configuration.
    pub fn config(&self) -> &AudioInputConfig {
        &self.config
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
    tx: mpsc::UnboundedSender<AudioBuffer>,
    stop_rx: std::sync::mpsc::Receiver<()>,
) -> Result<(), SpeechError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| SpeechError::AudioCaptureFailed("no input device found".to_string()))?;

    let input_cfg = device
        .default_input_config()
        .map_err(|e| SpeechError::AudioCaptureFailed(format!("get config failed: {}", e)))?;

    let stream_config = input_cfg.config();
    let shared = Arc::new(Mutex::new(Vec::<f32>::new()));

    let stream = build_stream_for_format(
        &device,
        &stream_config,
        input_cfg.sample_format(),
        Arc::clone(&shared),
        tx,
        config.sample_rate,
        config.buffer_duration_secs,
        config.energy_threshold,
    )?;

    stream
        .play()
        .map_err(|e| SpeechError::AudioCaptureFailed(format!("start stream failed: {}", e)))?;

    while stop_rx.try_recv().is_err() {
        thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_stream_for_format(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    shared: Arc<Mutex<Vec<f32>>>,
    tx: mpsc::UnboundedSender<AudioBuffer>,
    target_rate: u32,
    buffer_duration_secs: f32,
    energy_threshold: f32,
) -> Result<cpal::Stream, SpeechError> {
    let err_fn = |err| {
        eprintln!("audio input stream error: {}", err);
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
                    energy_threshold,
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
                    energy_threshold,
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
                    energy_threshold,
                    &shared,
                    &tx,
                );
            },
            err_fn,
            None,
        ),
        _ => {
            return Err(SpeechError::AudioCaptureFailed(
                "unsupported microphone sample format".to_string(),
            ));
        }
    }
    .map_err(|e| SpeechError::AudioCaptureFailed(format!("build stream failed: {}", e)))?;

    Ok(stream)
}

#[allow(clippy::too_many_arguments)]
fn process_input_chunk(
    input: &[f32],
    channels: u16,
    input_rate: u32,
    target_rate: u32,
    buffer_duration_secs: f32,
    energy_threshold: f32,
    shared: &Arc<Mutex<Vec<f32>>>,
    tx: &mpsc::UnboundedSender<AudioBuffer>,
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

        if calculate_rms(&processed) > energy_threshold {
            let _ = tx.send(AudioBuffer {
                samples: processed,
                sample_rate: target_rate,
                channels: 1,
            });
        }
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

/// Calculate RMS (root mean square) energy of audio samples.
fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_rms_returns_zero_for_empty() {
        let samples = vec![];
        assert_eq!(calculate_rms(&samples), 0.0);
    }

    #[test]
    fn calculate_rms_returns_expected_value() {
        let samples = vec![0.5, -0.5, 0.5, -0.5];
        let rms = calculate_rms(&samples);
        assert!((rms - 0.5).abs() < 0.0001);
    }

    #[test]
    fn calculate_rms_handles_silence() {
        let samples = vec![0.0; 1000];
        let rms = calculate_rms(&samples);
        assert_eq!(rms, 0.0);
    }

    #[test]
    fn downmix_to_mono_averages_channels() {
        let stereo = vec![1.0, -1.0, 0.5, 0.5];
        let mono = downmix_to_mono(&stereo, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn resample_linear_changes_length() {
        let input = vec![0.0; 48000];
        let output = resample_linear(&input, 48000, 16000);
        assert_eq!(output.len(), 16000);
    }

    #[test]
    fn audio_input_config_has_sensible_defaults() {
        let config = AudioInputConfig::default();
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.buffer_duration_secs, 3.0);
        assert_eq!(config.energy_threshold, 0.01);
    }
}
