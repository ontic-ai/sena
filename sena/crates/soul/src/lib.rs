//! SoulBox: identity schema, event log, personalization state.
//!
//! The Soul subsystem manages:
//! - Event log: append-only record of all Soul-relevant events
//! - Identity signals: behavioral patterns derived from platform observations
//! - Temporal patterns: time-based behavior models
//! - Preference learning: user preference inference and storage
//!
//! Schema v1 is backed by encrypted redb with migration support.

pub mod actor;
pub mod error;
pub mod redb_store;
pub mod schema;
pub mod store;
pub mod types;

pub use actor::SoulActor;
pub use bus::events::soul::{Verbosity, Warmth, WorkCadence};
pub use error::SoulError;
pub use redb_store::RedbSoulStore;
pub use schema::{SCHEMA_VERSION, SchemaV1, WindowEntry};
pub use store::SoulStore;
pub use types::{IdentitySignal, SoulEventRecord, SoulSummary, TemporalPattern};
