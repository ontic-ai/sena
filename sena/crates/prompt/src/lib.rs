//! Prompt composition subsystem.
//!
//! The Prompt subsystem manages:
//! - Typed prompt segment definitions carrying live data from Soul, Memory, CTP
//! - Stateless segment-based prompt assembly with no static strings
//! - Token budgeting and priority-based segment ordering
//! - Actor-based orchestration of prompt assembly
//!
//! ## Architecture
//!
//! - `PromptActor`: bus event-driven orchestrator
//! - `PromptComposer`: stateless assembler for typed segments
//! - `PromptSegment` enum: typed union of all segment kinds (SoulContext, LongTermMemory, etc.)
//! - No `PromptContext` or `ComposedPrompt` with provenance tracking (removed in favor of simpler design)
//!
//! ## Dependencies
//!
//! Allowed: bus, tokio, thiserror, tracing, serde
//! Forbidden: inference, memory, soul, ctp, platform (prompt only imports bus from workspace)
//!
//! ## Implementation Status
//!
//! Real typed prompt composition with no static strings. All content flows through
//! typed `PromptSegment` variants carrying live data.

pub mod actor;
pub mod composer;
pub mod error;
pub mod segment;

pub use actor::{PromptActor, PromptConfig};
pub use composer::{PromptComposer, PromptComposerTrait};
pub use error::PromptError;
pub use segment::{PromptSegment, ReflectionMode};
