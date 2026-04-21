//! Audio output (speaker playback) - minimal implementation for nested speech crate.
//!
//! Uses cpal for cross-platform speaker access. Simplified implementation
//! focused on playing f32 PCM buffers from TTS backends with format adaptation.

use crate::error::TtsError;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

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

/// Audio output stream manager.
///
/// Internally owns a dedicated playback thread so the actor can remain Send.
pub struct AudioOutputStream {
    #[allow(dead_code)]
    config: AudioOutputConfig,
    play_tx: Option<mpsc::UnboundedSender<AudioBuffer>>,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    playback_thread: Option<thread::JoinHandle<()>>,
}

impl AudioOutputStream {
    /// Create and start a new audio output stream.
    ///
    /// Returns the stream handle and a sender for audio buffers to play.
    pub fn start(config: AudioOutputConfig) -> Result<Self, TtsError> {
        let (play_tx, play_rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), TtsError>>();

        let worker_config = config.clone();
        let playback_thread = thread::spawn(move || {
            run_playback_loop(worker_config, play_rx, stop_rx, ready_tx);
        });

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                tracing::debug!("audio playback thread ready");
                Ok(Self {
                    config,
                    play_tx: Some(play_tx),
                    stop_tx: Some(stop_tx),
                    playback_thread: Some(playback_thread),
                })
            }
            Ok(Err(e)) => {
                tracing::error!("audio playback thread initialization failed: {}", e);
                let _ = stop_tx.send(());
                let _ = playback_thread.join();
                Err(e)
            }
            Err(_) => {
                tracing::error!("audio playback thread startup timed out");
                let _ = stop_tx.send(());
                let _ = playback_thread.join();
                Err(TtsError::BackendError(
                    "audio playback startup timed out".to_string(),
                ))
            }
        }
    }

    /// Queue an audio buffer for playback.
    ///
    /// Buffers are played in FIFO order. If the buffer's sample rate or channel count
    /// differs from the stream config, format adaptation will be applied.
    pub fn play(&self, buffer: AudioBuffer) -> Result<(), TtsError> {
        if let Some(tx) = &self.play_tx {
            tx.send(buffer).map_err(|_| {
                TtsError::BackendError("audio playback channel closed".to_string())
            })?;
            Ok(())
        } else {
            Err(TtsError::BackendError(
                "audio output stream not active".to_string(),
            ))
        }
    }

    /// Returns whether the playback thread is active.
    pub fn is_active(&self) -> bool {
        self.playback_thread.is_some()
    }
}

impl Drop for AudioOutputStream {
    fn drop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
        }
    }
}

fn run_playback_loop(
    config: AudioOutputConfig,
    mut play_rx: mpsc::UnboundedReceiver<AudioBuffer>,
    stop_rx: std::sync::mpsc::Receiver<()>,
    ready_tx: std::sync::mpsc::Sender<Result<(), TtsError>>,
) {
    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(d) => d,
        None => {
            let _ = ready_tx.send(Err(TtsError::BackendError(
                "no default output device".to_string(),
            )));
            return;
        }
    };

    let output_cfg = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(TtsError::BackendError(format!(
                "get output config failed: {}",
                e
            ))));
            return;
        }
    };

    let stream_config = StreamConfig {
        channels: config.channels,
        sample_rate: cpal::SampleRate(config.sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let playback_buffer = Arc::new(Mutex::new(Vec::<f32>::new()));

    let stream = match build_output_stream(
        &device,
        &stream_config,
        output_cfg.sample_format(),
        Arc::clone(&playback_buffer),
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = ready_tx.send(Err(TtsError::BackendError(format!(
            "stream play failed: {}",
            e
        ))));
        return;
    }

    let _ = ready_tx.send(Ok(()));

    loop {
        if stop_rx.try_recv().is_ok() {
            tracing::debug!("audio playback loop received stop signal");
            break;
        }

        match play_rx.try_recv() {
            Ok(buffer) => {
                let adapted = adapt_buffer_format(&buffer, &config);
                if let Ok(mut pb) = playback_buffer.lock() {
                    pb.extend_from_slice(&adapted.samples);
                    tracing::trace!(
                        "queued {} samples for playback",
                        adapted.samples.len()
                    );
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                tracing::debug!("audio playback channel disconnected");
                break;
            }
        }
    }

    drop(stream);
    tracing::debug!("audio playback loop exiting");
}

fn build_output_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    playback_buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, TtsError> {
    let channels = config.channels as usize;

    let err_fn = |err| {
        tracing::error!("audio output stream error: {}", err);
    };

    let stream = match format {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                write_output_data(data, &playback_buffer, channels);
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                let mut buffer = playback_buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                for sample in data.iter_mut() {
                    if buffer.is_empty() {
                        *sample = 0;
                    } else {
                        let f = buffer.remove(0);
                        *sample = (f * i16::MAX as f32) as i16;
                    }
                }
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                let mut buffer = playback_buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                for sample in data.iter_mut() {
                    if buffer.is_empty() {
                        *sample = u16::MAX / 2;
                    } else {
                        let f = buffer.remove(0);
                        *sample = ((f + 1.0) * 0.5 * u16::MAX as f32) as u16;
                    }
                }
            },
            err_fn,
            None,
        ),
        _ => {
            return Err(TtsError::BackendError(format!(
                "unsupported sample format: {:?}",
                format
            )))
        }
    };

    stream.map_err(|e| TtsError::BackendError(format!("build output stream failed: {}", e)))
}

fn write_output_data(data: &mut [f32], buffer: &Arc<Mutex<Vec<f32>>>, channels: usize) {
    let mut buffer = buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for frame in data.chunks_mut(channels) {
        if buffer.is_empty() {
            for sample in frame.iter_mut() {
                *sample = 0.0;
            }
        } else {
            let source_sample = buffer.remove(0);
            for sample in frame.iter_mut() {
                *sample = source_sample;
            }
        }
    }
}

/// Adapt audio buffer to target format (sample rate and channel count).
fn adapt_buffer_format(source: &AudioBuffer, target_config: &AudioOutputConfig) -> AudioBuffer {
    let samples = &source.samples;

    // Step 1: Channel adaptation
    let channel_adapted = if source.channels == target_config.channels {
        samples.clone()
    } else if source.channels == 1 && target_config.channels == 2 {
        // Mono to stereo: duplicate each sample
        samples
            .iter()
            .flat_map(|&sample| std::iter::repeat(sample).take(2))
            .collect()
    } else if source.channels == 2 && target_config.channels == 1 {
        // Stereo to mono: average pairs
        samples
            .chunks(2)
            .map(|pair| {
                if pair.len() == 2 {
                    (pair[0] + pair[1]) / 2.0
                } else {
                    pair[0]
                }
            })
            .collect()
    } else {
        tracing::warn!(
            "unsupported channel adaptation: {} -> {}",
            source.channels,
            target_config.channels
        );
        samples.clone()
    };

    // Step 2: Sample rate adaptation (simple linear interpolation)
    let samples_final = if source.sample_rate == target_config.sample_rate {
        channel_adapted
    } else {
        resample_linear(
            &channel_adapted,
            source.sample_rate,
            target_config.sample_rate,
        )
    };

    AudioBuffer {
        samples: samples_final,
        channels: target_config.channels,
        sample_rate: target_config.sample_rate,
    }
}

/// Simple linear interpolation resampler.
fn resample_linear(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let output_len = (samples.len() as f64 / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx0 = src_idx.floor() as usize;
        let idx1 = (idx0 + 1).min(samples.len() - 1);
        let frac = src_idx - idx0 as f64;

        let sample = samples[idx0] * (1.0 - frac) as f32 + samples[idx1] * frac as f32;
        output.push(sample);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_buffer_channel_mono_to_stereo() {
        let source = AudioBuffer {
            samples: vec![0.1, 0.2, 0.3],
            channels: 1,
            sample_rate: 16000,
        };
        let config = AudioOutputConfig {
            channels: 2,
            sample_rate: 16000,
            ..Default::default()
        };

        let adapted = adapt_buffer_format(&source, &config);
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
        let config = AudioOutputConfig {
            channels: 1,
            sample_rate: 16000,
            ..Default::default()
        };

        let adapted = adapt_buffer_format(&source, &config);
        assert_eq!(adapted.channels, 1);
        assert_eq!(adapted.samples.len(), 2);
        assert!((adapted.samples[0] - 0.15).abs() < 0.001);
        assert!((adapted.samples[1] - 0.35).abs() < 0.001);
    }

    #[test]
    fn audio_buffer_resample_upsampling() {
        let samples = vec![0.0, 1.0, 0.0];
        let resampled = resample_linear(&samples, 8000, 16000);
        assert!(resampled.len() > samples.len());
        assert!(resampled.len() <= samples.len() * 2 + 1);
    }

    #[test]
    fn audio_buffer_resample_downsampling() {
        let samples = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        let resampled = resample_linear(&samples, 16000, 8000);
        assert!(resampled.len() < samples.len());
        assert!(resampled.len() >= samples.len() / 2);
    }

    #[test]
    fn audio_buffer_no_adaptation_needed() {
        let source = AudioBuffer {
            samples: vec![0.1, 0.2, 0.3],
            channels: 1,
            sample_rate: 16000,
        };
        let config = AudioOutputConfig {
            channels: 1,
            sample_rate: 16000,
            ..Default::default()
        };

        let adapted = adapt_buffer_format(&source, &config);
        assert_eq!(adapted.samples, source.samples);
    }
}
