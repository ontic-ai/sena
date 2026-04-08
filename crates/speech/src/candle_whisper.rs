//! Candle-based Whisper STT implementation.
//!
//! NOTE(future): candle-transformers also supports YOLO (object recognition, pose estimation).
//! This candle infrastructure can be reused for vision-based context in future phases.
//! See: candle-transformers::models::yolo_v8 and yolo_v8_pose

use std::path::Path;

use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as m, audio, Config};
use rand::SeedableRng;
use tokenizers::Tokenizer;

use crate::SpeechError;

const SAMPLE_RATE: usize = 16_000;

/// Candle-based Whisper model wrapper.
pub struct CandleWhisperModel {
    model: WhisperModel,
    tokenizer: Tokenizer,
    config: Config,
    device: Device,
    language_token: Option<u32>,
}

impl CandleWhisperModel {
    /// Load a Whisper model from a local directory.
    ///
    /// Expects in `model_dir`:
    ///   - `model.safetensors` (or `model.gguf` for quantized)
    ///   - `config.json`
    ///   - `tokenizer.json`
    ///
    /// If `model_path_override` is provided, it is used as the exact model file path
    /// instead of scanning `model_dir`.
    pub fn load(model_dir: &Path, model_path_override: Option<&str>) -> Result<Self, SpeechError> {
        let device = Device::Cpu;

        let (config_path, tokenizer_path, model_path, is_quantized) =
            if let Some(override_path) = model_path_override {
                let override_path = Path::new(override_path);
                let dir = override_path.parent().ok_or_else(|| {
                    SpeechError::SttInitFailed("model path has no parent directory".to_string())
                })?;
                let config = dir.join("config.json");
                let tokenizer = dir.join("tokenizer.json");
                let is_gguf = override_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e == "gguf")
                    .unwrap_or(false);
                (config, tokenizer, override_path.to_path_buf(), is_gguf)
            } else {
                let config = model_dir.join("config.json");
                let tokenizer = model_dir.join("tokenizer.json");
                let gguf_path = model_dir.join("model.gguf");
                let safetensors_path = model_dir.join("model.safetensors");

                if gguf_path.exists() {
                    (config, tokenizer, gguf_path, true)
                } else if safetensors_path.exists() {
                    (config, tokenizer, safetensors_path, false)
                } else {
                    return Err(SpeechError::SttInitFailed(
                        "no model.gguf or model.safetensors found in model directory".to_string(),
                    ));
                }
            };

        if !config_path.exists() {
            return Err(SpeechError::SttInitFailed(format!(
                "config.json not found at {:?}",
                config_path
            )));
        }

        if !tokenizer_path.exists() {
            return Err(SpeechError::SttInitFailed(format!(
                "tokenizer.json not found at {:?}",
                tokenizer_path
            )));
        }

        let config_content = std::fs::read_to_string(&config_path)
            .map_err(|e| SpeechError::SttInitFailed(format!("read config.json: {}", e)))?;
        let config: Config = serde_json::from_str(&config_content)
            .map_err(|e| SpeechError::SttInitFailed(format!("parse config.json: {}", e)))?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| SpeechError::SttInitFailed(format!("load tokenizer: {}", e)))?;

        let model = if is_quantized {
            let vb = candle_transformers::quantized_var_builder::VarBuilder::from_gguf(
                &model_path,
                &device,
            )
            .map_err(|e| SpeechError::SttInitFailed(format!("load quantized model: {}", e)))?;
            WhisperModel::Quantized(
                m::quantized_model::Whisper::load(&vb, config.clone()).map_err(|e| {
                    SpeechError::SttInitFailed(format!("build quantized whisper: {}", e))
                })?,
            )
        } else {
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[model_path], candle_core::DType::F32, &device)
                    .map_err(|e| SpeechError::SttInitFailed(format!("load safetensors: {}", e)))?
            };
            WhisperModel::Normal(
                m::model::Whisper::load(&vb, config.clone()).map_err(|e| {
                    SpeechError::SttInitFailed(format!("build whisper model: {}", e))
                })?,
            )
        };

        let language_token = tokenizer.token_to_id("<|en|>");

        Ok(Self {
            model,
            tokenizer,
            config,
            device,
            language_token,
        })
    }

    /// Transcribe PCM audio samples (f32 normalized, 16kHz, mono) to text.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, SpeechError> {
        // Validate audio input - need at least 1 second of audio for meaningful transcription
        if samples.len() < SAMPLE_RATE {
            return Err(SpeechError::TranscriptionFailed(
                "insufficient audio samples (< 1 second)".to_string(),
            ));
        }

        let n_fft = 400;
        let n_mels = self.config.num_mel_bins;
        let filters = mel_filters(n_mels, n_fft, SAMPLE_RATE as u32);

        let mel = audio::pcm_to_mel(&self.config, samples, &filters);
        let mel_len = mel.len();

        // Validate mel spectrogram has content
        if mel_len == 0 || mel_len < n_mels {
            return Err(SpeechError::TranscriptionFailed(
                "mel spectrogram generation failed or insufficient frames".to_string(),
            ));
        }

        let mel = Tensor::from_vec(mel, (1, n_mels, mel_len / n_mels), &self.device)
            .map_err(|e| SpeechError::TranscriptionFailed(format!("mel tensor: {}", e)))?;

        let transcribe_task = self.tokenizer.token_to_id(m::TRANSCRIBE_TOKEN);
        let mut decoder = Decoder::new(
            &mut self.model,
            &self.tokenizer,
            299792458, // seed
            &self.device,
            self.language_token,
            transcribe_task,
            false, // timestamps off
            false, // verbose off
        )
        .map_err(|e| SpeechError::TranscriptionFailed(format!("decoder init: {}", e)))?;

        let segments = decoder
            .run(&mel)
            .map_err(|e| SpeechError::TranscriptionFailed(format!("decoder run: {}", e)))?;

        let text: String = segments
            .iter()
            .map(|s| s.dr.text.as_str())
            .collect::<Vec<_>>()
            .join("");

        Ok(text.trim().to_string())
    }
}

/// Compute mel filterbank matrix at runtime.
///
/// Returns a flat vector of size `n_mels * (n_fft/2 + 1)` with triangular filters
/// in row-major order, following the Slaney normalization convention.
fn mel_filters(n_mels: usize, n_fft: usize, sample_rate: u32) -> Vec<f32> {
    let n_freqs = n_fft / 2 + 1;
    let nyquist = sample_rate as f32 / 2.0;

    let hz_to_mel = |hz: f32| 2595.0 * (1.0 + hz / 700.0).log10();
    let mel_to_hz = |mel: f32| 700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0);

    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(nyquist);

    let mel_points: Vec<f32> = (0..=(n_mels + 1))
        .map(|i| mel_min + (mel_max - mel_min) * i as f32 / (n_mels + 1) as f32)
        .collect();

    let bin_points: Vec<f32> = mel_points
        .iter()
        .map(|&m| mel_to_hz(m) / nyquist * (n_freqs as f32 - 1.0))
        .collect();

    let mut filters = vec![0.0f32; n_mels * n_freqs];
    for m in 0..n_mels {
        for k in 0..n_freqs {
            let k_f32 = k as f32;
            let lower = bin_points[m];
            let center = bin_points[m + 1];
            let upper = bin_points[m + 2];
            let val = if k_f32 >= lower && k_f32 <= center {
                (k_f32 - lower) / (center - lower)
            } else if k_f32 > center && k_f32 <= upper {
                (upper - k_f32) / (upper - center)
            } else {
                0.0
            };
            filters[m * n_freqs + k] = val;
        }
    }

    // Slaney normalization
    for m in 0..n_mels {
        let hz_low = mel_to_hz(mel_points[m]);
        let hz_high = mel_to_hz(mel_points[m + 2]);
        let width = hz_high - hz_low;
        if width > 0.0 {
            for k in 0..n_freqs {
                filters[m * n_freqs + k] *= 2.0 / width;
            }
        }
    }

    filters
}

/// Unified Whisper model enum (normal or quantized).
enum WhisperModel {
    Normal(m::model::Whisper),
    Quantized(m::quantized_model::Whisper),
}

impl WhisperModel {
    fn encoder_forward(&mut self, x: &Tensor, flush: bool) -> candle_core::Result<Tensor> {
        match self {
            Self::Normal(m) => m.encoder.forward(x, flush),
            Self::Quantized(m) => m.encoder.forward(x, flush),
        }
    }

    fn decoder_forward(
        &mut self,
        x: &Tensor,
        xa: &Tensor,
        flush: bool,
    ) -> candle_core::Result<Tensor> {
        match self {
            Self::Normal(m) => m.decoder.forward(x, xa, flush),
            Self::Quantized(m) => m.decoder.forward(x, xa, flush),
        }
    }

    fn decoder_final_linear(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        match self {
            Self::Normal(m) => m.decoder.final_linear(x),
            Self::Quantized(m) => m.decoder.final_linear(x),
        }
    }

    fn config(&self) -> &Config {
        match self {
            Self::Normal(m) => &m.config,
            Self::Quantized(m) => &m.config,
        }
    }
}

/// Decoded segment with timing and text.
#[derive(Debug, Clone)]
struct Segment {
    start: f64,
    duration: f64,
    dr: DecodingResult,
}

/// Decoding result with transcribed text, token sequence, and timing.
#[derive(Debug, Clone)]
struct DecodingResult {
    #[allow(dead_code)]
    tokens: Vec<u32>,
    text: String,
    avg_logprob: f64,
    no_speech_prob: f64,
    #[allow(dead_code)]
    temperature: f64,
    #[allow(dead_code)]
    compression_ratio: f64,
}

/// Whisper decoder orchestrator (adapted from candle-examples).
struct Decoder<'a> {
    model: &'a mut WhisperModel,
    rng: rand::rngs::StdRng,
    task: Option<u32>,
    timestamps: bool,
    verbose: bool,
    tokenizer: &'a Tokenizer,
    suppress_tokens: Tensor,
    sot_token: u32,
    #[allow(dead_code)]
    transcribe_token: u32,
    #[allow(dead_code)]
    translate_token: u32,
    eot_token: u32,
    no_speech_token: u32,
    no_timestamps_token: u32,
    language_token: Option<u32>,
}

impl<'a> Decoder<'a> {
    fn normalize_logits_rank1(logits: Tensor) -> candle_core::Result<Tensor> {
        match logits.rank() {
            3 => {
                let (_, _, vocab) = logits.dims3()?;
                logits.reshape((vocab,))
            }
            2 => {
                let (_, vocab) = logits.dims2()?;
                logits.reshape((vocab,))
            }
            1 => Ok(logits),
            rank => Err(candle_core::Error::msg(format!(
                "unexpected logits rank {}, expected 1/2/3",
                rank
            ))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        model: &'a mut WhisperModel,
        tokenizer: &'a Tokenizer,
        seed: u64,
        device: &Device,
        language_token: Option<u32>,
        task: Option<u32>,
        timestamps: bool,
        verbose: bool,
    ) -> candle_core::Result<Self> {
        let no_timestamps_token = tokenizer.token_to_id(m::NO_TIMESTAMPS_TOKEN).unwrap_or(0);
        let suppress_tokens: Vec<f32> = (0..model.config().vocab_size as u32)
            .map(|i| {
                if model.config().suppress_tokens.contains(&i) {
                    f32::NEG_INFINITY
                } else {
                    0f32
                }
            })
            .collect();
        let suppress_tokens = Tensor::new(suppress_tokens.as_slice(), device)?;
        let sot_token = tokenizer.token_to_id(m::SOT_TOKEN).unwrap_or(0);
        let transcribe_token = tokenizer.token_to_id(m::TRANSCRIBE_TOKEN).unwrap_or(0);
        let translate_token = tokenizer.token_to_id(m::TRANSLATE_TOKEN).unwrap_or(0);
        let eot_token = tokenizer.token_to_id(m::EOT_TOKEN).unwrap_or(0);
        let no_speech_token = tokenizer.token_to_id("<|nospeech|>").unwrap_or(0);

        Ok(Self {
            model,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
            task,
            timestamps,
            verbose,
            tokenizer,
            suppress_tokens,
            sot_token,
            transcribe_token,
            translate_token,
            eot_token,
            no_speech_token,
            language_token,
            no_timestamps_token,
        })
    }

    fn decode(&mut self, mel: &Tensor, t: f64) -> candle_core::Result<DecodingResult> {
        let model = &mut self.model;
        let audio_features = model.encoder_forward(mel, true)?;
        if self.verbose {
            tracing::debug!("audio features: {:?}", audio_features.dims());
        }

        let sample_len = model.config().max_target_positions / 2;
        let mut sum_logprob = 0f64;
        let mut no_speech_prob = f64::NAN;
        let mut tokens = vec![self.sot_token];
        if let Some(language_token) = self.language_token {
            tokens.push(language_token);
        }
        if let Some(task) = self.task {
            tokens.push(task);
        }
        if !self.timestamps {
            tokens.push(self.no_timestamps_token);
        }

        for i in 0..sample_len {
            let tokens_t = Tensor::new(tokens.as_slice(), mel.device())?;

            let tokens_t = tokens_t.unsqueeze(0)?;
            let ys = model.decoder_forward(&tokens_t, &audio_features, i == 0)?;

            let (_, seq_len, _) = ys.dims3()?;
            let logits = model.decoder_final_linear(&ys.i((..1, seq_len - 1..))?)?;
            let logits = Self::normalize_logits_rank1(logits)?;
            let logits = logits.broadcast_add(&self.suppress_tokens)?;

            let next_token = if t > 0f64 {
                let prs = candle_nn::ops::softmax(&(&logits / t)?, 0)?;
                let logits_v: Vec<f32> = prs.to_vec1()?;
                let distr = rand::distr::weighted::WeightedIndex::new(&logits_v)
                    .map_err(candle_core::Error::wrap)?;
                use rand::distr::Distribution;
                distr.sample(&mut self.rng) as u32
            } else {
                let logits_v: Vec<f32> = logits.to_vec1()?;
                logits_v
                    .iter()
                    .enumerate()
                    .max_by(|(_, u): &(usize, &f32), (_, v): &(usize, &f32)| u.total_cmp(v))
                    .map(|(i, _)| i as u32)
                    .unwrap_or(0)
            };

            tokens.push(next_token);
            let prob_tensor =
                candle_nn::ops::softmax(&logits, candle_core::D::Minus1)?.i(next_token as usize)?;
            // Indexing a rank-1 tensor with .i() produces a rank-0 scalar - use to_scalar()
            let prob = prob_tensor.to_scalar::<f32>()? as f64;
            if i == 0 {
                let no_speech_tensor = candle_nn::ops::softmax(&logits, candle_core::D::Minus1)?
                    .i(self.no_speech_token as usize)?;
                no_speech_prob = no_speech_tensor.to_scalar::<f32>()? as f64;
            }
            sum_logprob += prob.ln();
            if next_token == self.eot_token || tokens.len() > model.config().max_target_positions {
                break;
            }
        }

        let text = self
            .tokenizer
            .decode(&tokens, true)
            .map_err(candle_core::Error::msg)?;
        let avg_logprob = sum_logprob / tokens.len() as f64;

        Ok(DecodingResult {
            tokens,
            text,
            avg_logprob,
            no_speech_prob,
            temperature: t,
            compression_ratio: f64::NAN,
        })
    }

    fn decode_with_fallback(&mut self, segment: &Tensor) -> candle_core::Result<DecodingResult> {
        for (i, &t) in m::TEMPERATURES.iter().enumerate() {
            let dr = self.decode(segment, t)?;
            let needs_fallback = dr.compression_ratio > m::COMPRESSION_RATIO_THRESHOLD
                || dr.avg_logprob < m::LOGPROB_THRESHOLD;
            if !needs_fallback || i == m::TEMPERATURES.len() - 1 {
                return Ok(dr);
            }
        }
        unreachable!()
    }

    fn run(&mut self, mel: &Tensor) -> candle_core::Result<Vec<Segment>> {
        let (_, _, content_frames) = mel.dims3()?;
        let mut seek = 0;
        let mut segments = vec![];
        while seek < content_frames {
            let time_offset = (seek * m::HOP_LENGTH) as f64 / SAMPLE_RATE as f64;
            let segment_size = usize::min(content_frames - seek, m::N_FRAMES);
            let mel_segment = mel.narrow(2, seek, segment_size)?;
            let segment_duration = (segment_size * m::HOP_LENGTH) as f64 / SAMPLE_RATE as f64;
            let dr = self.decode_with_fallback(&mel_segment)?;

            seek += segment_size;
            if dr.no_speech_prob > m::NO_SPEECH_THRESHOLD && dr.avg_logprob < m::LOGPROB_THRESHOLD {
                tracing::debug!("no speech detected, skipping segment");
                continue;
            }
            let segment = Segment {
                start: time_offset,
                duration: segment_duration,
                dr,
            };
            if self.verbose {
                tracing::info!(
                    "{:.1}s -- {:.1}s: {}",
                    segment.start,
                    segment.start + segment.duration,
                    segment.dr.text,
                );
            }
            segments.push(segment);
        }
        Ok(segments)
    }
}
