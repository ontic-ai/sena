//! ech0 adapter — memory ingestion and retrieval

pub mod encrypted_store;
pub mod error;

pub use encrypted_store::EncryptedStore;
pub use error::MemoryError;
