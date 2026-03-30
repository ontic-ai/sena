//! Continuous Thought Processing — context assembly and thought triggering.
//!
//! The CTP layer sits between platform signals and higher-level reasoning.
//! It implements the pipeline:
//!
//! Platform Events → Signal Buffer → Context Assembler → Trigger Gate → ThoughtEvent
//!
//! ## Architecture
//!
//! - `SignalBuffer`: Rolling time-window accumulator for platform events
//! - `ContextAssembler`: Transforms buffer state into typed ContextSnapshot
//! - `TriggerGate`: Time-based (Phase 1) trigger logic
//! - `CTPActor`: Orchestrates the pipeline, communicates via bus

pub mod context_assembler;
pub mod ctp_actor;
pub mod signal_buffer;
pub mod trigger_gate;

pub use ctp_actor::CTPActor;
