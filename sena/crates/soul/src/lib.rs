//! SoulBox: identity schema, event log, personalization state.
//!
//! The Soul subsystem manages:
//! - Event log: append-only record of all Soul-relevant events
//! - Identity signals: behavioral patterns derived from platform observations
//! - Temporal patterns: time-based behavior models
//! - Preference learning: user preference inference and storage
//!
//! This is a stub implementation that defines the trait abstraction and actor
//! skeleton. Concrete encrypted database implementation will follow.

pub mod actor;
pub mod error;
pub mod store;
pub mod types;

pub use actor::SoulActor;
pub use error::SoulError;
pub use store::SoulStore;
pub use types::{IdentitySignal, SoulEventRecord, SoulSummary, TemporalPattern};
