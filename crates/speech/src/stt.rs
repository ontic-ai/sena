//! STT backend trait, per-backend implementations, and construction factory.
//!
//! # Module layout
//! - `backend_trait` — `SttBackend` trait and `SttEvent` enum
//! - `mock_backend`  — `MockSttBackend` (no model, for tests)
//! - `whisper_backend` — `WhisperSttBackend` (candle inference)
//! - `sherpa_backend`  — `SherpaSttBackend` (sherpa-onnx Zipformer)
//! - `parakeet_backend` — `ParakeetSttBackend` (NVIDIA Parakeet-EOU ONNX)
//! - `factory`         — `build_stt_backend()` async construction helper

pub mod backend_trait;
pub mod factory;
pub(crate) mod mock_backend;
pub(crate) mod parakeet_backend;
pub(crate) mod sherpa_backend;
pub(crate) mod whisper_backend;

pub use backend_trait::{SttBackend, SttEvent};
pub use factory::build_stt_backend;
