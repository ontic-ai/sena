//! SoulBox: identity schema, event log, personalization state

pub mod encrypted_db;
pub mod error;

pub use encrypted_db::EncryptedDb;
pub use error::SoulError;
