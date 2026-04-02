//! Audio output (speaker playback) - cross-platform.
//!
//! Uses cpal to play synthesized PCM audio on the default output device.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SupportedStreamConfig};

use crate::error::SpeechError;

pub struct AudioOutput {
    device: Device,
    config: SupportedStreamConfig,
}

impl AudioOutput {
    pub fn new() -> Result<Self, SpeechError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| SpeechError::AudioPlaybackFailed("no output device".to_string()))?;

        let config = device
            .default_output_config()
            .map_err(|e| SpeechError::AudioPlaybackFailed(format!("config query failed: {e}")))?;

        Ok(Self { device, config })
    }

    pub fn play_pcm16_mono_22050(&self, samples: &[i16]) -> Result<(), SpeechError> {
        if samples.is_empty() {
            return Ok(());
        }

        let output_sample_rate = self.config.sample_rate().0;
        let channels = usize::from(self.config.channels());
        let resampled = if output_sample_rate == 22_050 {
            samples.to_vec()
        } else {
            resample_pcm_i16(samples, 22_050, output_sample_rate)
        };

        match self.config.sample_format() {
            SampleFormat::F32 => self.play_f32_stream(&resampled, channels, output_sample_rate),
            SampleFormat::I16 => self.play_i16_stream(&resampled, channels, output_sample_rate),
            SampleFormat::U16 => self.play_u16_stream(&resampled, channels, output_sample_rate),
            _ => Err(SpeechError::AudioPlaybackFailed(
                "unsupported output sample format".to_string(),
            )),
        }
    }

    fn play_f32_stream(
        &self,
        mono_samples: &[i16],
        channels: usize,
        sample_rate_hz: u32,
    ) -> Result<(), SpeechError> {
        let idx = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let sample_data = Arc::new(mono_samples.to_vec());

        let idx_cb = Arc::clone(&idx);
        let done_cb = Arc::clone(&done);
        let samples_cb = Arc::clone(&sample_data);

        let stream = self
            .device
            .build_output_stream(
                &self.config.config(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    write_frames_f32(data, channels, &samples_cb, &idx_cb, &done_cb)
                },
                move |err| {
                    eprintln!("audio output error: {err}");
                },
                None,
            )
            .map_err(|e| SpeechError::AudioPlaybackFailed(format!("build stream failed: {e}")))?;

        stream
            .play()
            .map_err(|e| SpeechError::AudioPlaybackFailed(format!("stream play failed: {e}")))?;

        wait_until_done(
            &done,
            sample_data.len(),
            sample_rate_hz,
            "playback timed out for f32 stream",
        )
    }

    fn play_i16_stream(
        &self,
        mono_samples: &[i16],
        channels: usize,
        sample_rate_hz: u32,
    ) -> Result<(), SpeechError> {
        let idx = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let sample_data = Arc::new(mono_samples.to_vec());

        let idx_cb = Arc::clone(&idx);
        let done_cb = Arc::clone(&done);
        let samples_cb = Arc::clone(&sample_data);

        let stream = self
            .device
            .build_output_stream(
                &self.config.config(),
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    write_frames_i16(data, channels, &samples_cb, &idx_cb, &done_cb)
                },
                move |err| {
                    eprintln!("audio output error: {err}");
                },
                None,
            )
            .map_err(|e| SpeechError::AudioPlaybackFailed(format!("build stream failed: {e}")))?;

        stream
            .play()
            .map_err(|e| SpeechError::AudioPlaybackFailed(format!("stream play failed: {e}")))?;

        wait_until_done(
            &done,
            sample_data.len(),
            sample_rate_hz,
            "playback timed out for i16 stream",
        )
    }

    fn play_u16_stream(
        &self,
        mono_samples: &[i16],
        channels: usize,
        sample_rate_hz: u32,
    ) -> Result<(), SpeechError> {
        let idx = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let sample_data = Arc::new(mono_samples.to_vec());

        let idx_cb = Arc::clone(&idx);
        let done_cb = Arc::clone(&done);
        let samples_cb = Arc::clone(&sample_data);

        let stream = self
            .device
            .build_output_stream(
                &self.config.config(),
                move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                    write_frames_u16(data, channels, &samples_cb, &idx_cb, &done_cb)
                },
                move |err| {
                    eprintln!("audio output error: {err}");
                },
                None,
            )
            .map_err(|e| SpeechError::AudioPlaybackFailed(format!("build stream failed: {e}")))?;

        stream
            .play()
            .map_err(|e| SpeechError::AudioPlaybackFailed(format!("stream play failed: {e}")))?;

        wait_until_done(
            &done,
            sample_data.len(),
            sample_rate_hz,
            "playback timed out for u16 stream",
        )
    }
}

fn write_frames_f32(
    data: &mut [f32],
    channels: usize,
    samples: &Arc<Vec<i16>>,
    idx: &Arc<AtomicUsize>,
    done: &Arc<AtomicBool>,
) {
    for frame in data.chunks_mut(channels) {
        let sample_idx = idx.fetch_add(1, Ordering::SeqCst);
        let value = if sample_idx < samples.len() {
            f32::from(samples[sample_idx]) / f32::from(i16::MAX)
        } else {
            done.store(true, Ordering::SeqCst);
            0.0
        };

        for sample in frame {
            *sample = value;
        }
    }
}

fn write_frames_i16(
    data: &mut [i16],
    channels: usize,
    samples: &Arc<Vec<i16>>,
    idx: &Arc<AtomicUsize>,
    done: &Arc<AtomicBool>,
) {
    for frame in data.chunks_mut(channels) {
        let sample_idx = idx.fetch_add(1, Ordering::SeqCst);
        let value = if sample_idx < samples.len() {
            samples[sample_idx]
        } else {
            done.store(true, Ordering::SeqCst);
            0
        };

        for sample in frame {
            *sample = value;
        }
    }
}

fn write_frames_u16(
    data: &mut [u16],
    channels: usize,
    samples: &Arc<Vec<i16>>,
    idx: &Arc<AtomicUsize>,
    done: &Arc<AtomicBool>,
) {
    for frame in data.chunks_mut(channels) {
        let sample_idx = idx.fetch_add(1, Ordering::SeqCst);
        let value = if sample_idx < samples.len() {
            let shifted = i32::from(samples[sample_idx]) + 32768;
            shifted as u16
        } else {
            done.store(true, Ordering::SeqCst);
            32768
        };

        for sample in frame {
            *sample = value;
        }
    }
}

fn wait_until_done(
    done: &Arc<AtomicBool>,
    samples_len: usize,
    sample_rate_hz: u32,
    timeout_message: &str,
) -> Result<(), SpeechError> {
    let expected_ms = ((samples_len as f64 / sample_rate_hz as f64) * 1000.0) as u64;
    let timeout = Duration::from_millis(expected_ms.saturating_add(3000));
    let start = std::time::Instant::now();

    while !done.load(Ordering::SeqCst) {
        if start.elapsed() > timeout {
            return Err(SpeechError::AudioPlaybackFailed(
                timeout_message.to_string(),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}

fn resample_pcm_i16(samples: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if samples.is_empty() || src_rate == dst_rate {
        return samples.to_vec();
    }

    let ratio = dst_rate as f32 / src_rate as f32;
    let out_len = (samples.len() as f32 * ratio).max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = (i as f32) / ratio;
        let src_idx = src_pos as usize;
        out.push(samples[src_idx.min(samples.len() - 1)]);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_identity_when_rates_match() {
        let source = vec![1i16, 2, 3, 4, 5];
        let out = resample_pcm_i16(&source, 22_050, 22_050);
        assert_eq!(out, source);
    }

    #[test]
    fn resample_changes_length_for_rate_conversion() {
        let source = vec![100i16; 22050];
        let out = resample_pcm_i16(&source, 22_050, 44_100);
        assert_eq!(out.len(), 44_100);
    }

    #[test]
    fn audio_output_initializes_when_device_exists() {
        if let Ok(output) = AudioOutput::new() {
            assert!(output.config.sample_rate().0 > 0);
            assert!(output.config.channels() > 0);
        }
    }
}
