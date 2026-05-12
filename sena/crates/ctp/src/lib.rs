//! CTP (Continuous Thought Processing) subsystem.
//!
//! The CTP subsystem is Sena's observation and reasoning cortex:
//! - Ingests signals from platform, soul, and other subsystems
//! - Assembles multi-modal context snapshots
//! - Applies trigger gating logic to decide when to think proactively
//! - Emits ThoughtEvent with full context for inference consumption
//!
//! ## Architecture
//!
//! CTP is signal-driven and actor-based:
//! - `CtpSignal`: typed union of all observable signal types
//! - `CtpActor`: signal buffer, snapshot assembler, trigger evaluator
//! - `SignalBuffer`: rolling time-window accumulator for platform events
//! - `ContextAssembler`: transforms signal buffer into ContextSnapshot
//! - `TriggerGate`: enforces proactive-thought cooldown over raw snapshots
//!
//! ## Dependencies
//!
//! Allowed: bus, platform, soul, thiserror, tracing, tokio, serde
//! Forbidden: inference, memory (CTP triggers inference; it does not invoke it)
//!
//! ## Privacy Boundary
//!
//! CTP processes privacy-safe signals only. Raw keystroke content, clipboard text,
//! and other sensitive data never flow through CTP's signal buffer.

pub mod actor;
pub mod context_assembler;
pub mod error;
pub mod signal;
pub mod signal_buffer;
pub mod transparency_query;
pub mod trigger_gate;

pub use actor::CtpActor;
pub use context_assembler::ContextAssembler;
pub use error::CtpError;
pub use signal::CtpSignal;
pub use signal_buffer::SignalBuffer;
pub use trigger_gate::TriggerGate;
