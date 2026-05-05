//! SoulBox: identity schema, event log, personalization state

pub mod actor;
pub mod encrypted_db;
pub mod error;
pub mod redacted;
pub mod schema;

mod distillation;
mod preference_learning;
mod summary_assembler;
mod temporal_model;

pub use actor::SoulActor;
pub use distillation::DistillationEngine;
pub use encrypted_db::EncryptedDb;
pub use error::SoulError;
pub use preference_learning::PreferenceLearner;
pub use redacted::Redacted;
pub use schema::apply_schema;
pub use summary_assembler::SummaryAssembler;
pub use temporal_model::TemporalModel;
