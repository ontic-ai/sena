//! ech0 adapter — memory ingestion and retrieval

pub mod actor;
pub mod embedder;
pub mod encrypted_store;
pub mod error;
pub mod extractor;
pub mod redacted;
pub mod transparency_query;
pub mod working_memory;

pub use actor::MemoryActor;
pub use embedder::SenaEmbedder;
pub use encrypted_store::EncryptedStore;
pub use error::MemoryError;
pub use extractor::SenaExtractor;
pub use redacted::Redacted;
pub use transparency_query::handle_transparency_query;
pub use working_memory::{InferenceExchange, WorkingMemory};
