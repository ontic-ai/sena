//! llama-cpp-rs wrapper, model manager, inference queue

pub mod actor;
pub mod discovery;
pub mod error;
pub mod queue;
pub mod registry;
mod transparency_query;

// Re-export from infer crate
pub use infer::{
    BackendType, ChatTemplate, ExtractionResult, InferError, InferenceParams, MockBackend,
    MockConfig, ModelRegistry,
};

// Re-export InferenceBackend as LlmBackend for backward compatibility
pub use infer::InferenceBackend as LlmBackend;

// Re-export LlamaBackend - available with default features in infer crate
pub use infer::LlamaBackend;

// Re-export from local modules
pub use actor::InferenceActor;
pub use discovery::discover_models;
pub use error::InferenceError;
pub use queue::{InferenceQueue, WorkKind};

/// Suppress all llama.cpp log output to prevent TUI terminal corruption.
///
/// Call this before entering any full-screen TUI mode. llama.cpp writes
/// model-load progress and debug info directly to stderr via C callbacks,
/// which corrupts ratatui's alternate screen buffer. This call installs a
/// no-op log callback globally, silencing all llama.cpp output permanently
/// for the lifetime of the process.
pub fn suppress_llama_logs() {
    infer::suppress_llama_logs();
}
