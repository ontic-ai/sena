//! Prompt composition subsystem.
//!
//! The Prompt subsystem manages:
//! - Typed prompt segment definitions (SystemPersona, MemoryContext, etc.)
//! - Segment-based prompt assembly with provenance tracking
//! - Token budgeting and limit enforcement
//! - Trace generation showing how prompts were composed
//! - Actor-based orchestration of prompt assembly
//!
//! ## Architecture
//!
//! - `PromptActor`: bus event-driven orchestrator
//! - `PromptComposer` trait: strategy interface for prompt assembly
//! - `PromptSegment` enum: typed union of all segment kinds
//! - `PromptContext`: aggregated context from Soul, Memory, CTP, and user input
//! - `ComposedPrompt`: final prompt with text, trace, and token count
//! - `PromptTrace`: provenance record for debugging and transparency
//!
//! ## Dependencies
//!
//! Allowed: bus, tokio, thiserror, tracing, serde
//! Forbidden: inference, memory, soul, ctp, platform (prompt only imports bus from workspace)
//!
//! ## BONES Implementation Status
//!
//! This is a stub implementation. The composer returns placeholder text with
//! full provenance tracking. Real implementations will perform sophisticated
//! segment assembly, compression, and token optimization.
//!
//! No static system prompt strings exist in this implementation beyond generic
//! BONES placeholder markers used for testing and development transparency.

pub mod actor;
pub mod composer;
pub mod error;
pub mod segment;
pub mod types;

pub use actor::{PromptActor, PromptConfig};
pub use composer::{PromptComposer, StubComposer};
pub use error::PromptError;
pub use segment::PromptSegment;
pub use types::{ComposedPrompt, PromptContext, PromptTrace, Provenance, SegmentTrace};
