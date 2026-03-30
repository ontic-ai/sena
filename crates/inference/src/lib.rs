//! llama-cpp-rs wrapper, model manager, inference queue

pub mod actor;
pub mod backend;
pub mod discovery;
pub mod error;
pub mod llama_backend;
pub mod manifest;
pub mod mock_backend;
pub mod queue;
pub mod registry;

pub use actor::InferenceActor;
pub use backend::{BackendType, InferenceParams, LlmBackend};
pub use discovery::discover_models;
pub use error::InferenceError;
pub use llama_backend::LlamaBackend;
pub use mock_backend::MockBackend;
pub use queue::{InferenceQueue, WorkKind};
pub use registry::ModelRegistry;
