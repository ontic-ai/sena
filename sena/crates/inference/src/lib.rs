//! Inference subsystem: backend trait, async token streaming, and actor.

pub mod actor;
pub mod backend;
pub mod error;
pub mod filter;
pub mod mock;
pub mod queue;
pub mod registry;
pub mod stream;
pub mod types;

pub use actor::InferenceActor;
pub use backend::InferenceBackend;
pub use error::InferenceError;
pub use filter::OutputFilter;
pub use mock::{MockBackend, MockConfig};
pub use queue::{InferenceQueue, WorkItem, WorkKind};
pub use registry::{ModelInfo, ModelRegistry, discover_models};
pub use stream::InferenceStream;
pub use types::{BackendType, InferenceParams};

// Re-export infer backend types for use by inference subsystem components.
pub use infer::{
    ChatTemplate, ExtractionResult, InferError as BackendError, InferenceBackend as LlmBackend,
    LlamaBackend,
};
