//! Inference subsystem: backend trait, async token streaming, and actor.

pub mod actor;
pub mod backend;
pub mod discovery;
pub mod error;
pub mod filter;
pub mod llama_loader;
pub mod mock;
pub mod queue;
pub mod registry;
pub mod stream;
pub mod types;

pub use actor::{EmbedRequest, InferenceActor};
pub use backend::InferenceBackend;
pub use discovery::discover_models;
pub use error::InferenceError;
pub use filter::OutputFilter;
pub use llama_loader::build_loaded_llama_backend;
pub use llama_loader::build_loaded_embed_backend;
pub use llama_loader::preferred_llama_backend;
pub use mock::{MockBackend, MockConfig};
pub use queue::{InferenceQueue, WorkItem, WorkKind};
pub use registry::{ModelInfo, ModelRegistry};
pub use stream::InferenceStream;
pub use types::{BackendType, InferenceParams};

// Re-export infer backend types for use by inference subsystem components.
pub use infer::{
    ChatTemplate, ExtractionResult, InferError as BackendError, InferenceBackend as LlmBackend,
    LlamaBackend,
};

/// Suppress all llama.cpp log output to prevent TUI terminal corruption.
///
/// Call this before entering any full-screen TUI mode or early in boot.
/// llama.cpp writes model-load progress and debug info directly to stderr
/// via C callbacks, which corrupts ratatui's alternate screen buffer.
/// This installs a no-op log callback globally, silencing all llama.cpp
/// output permanently for the lifetime of the process.
pub fn suppress_llama_logs() {
    infer::suppress_llama_logs();
}
