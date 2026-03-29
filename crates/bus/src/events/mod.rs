//! Typed event definitions.
//!
//! This is the single source of truth for all event types in Sena.
//! Events are organized by domain. All events are Clone + Send + 'static
//! and carry no logic — they are pure data.

pub mod ctp;
pub mod platform;
pub mod system;

pub use ctp::*;
pub use platform::*;
pub use system::*;
