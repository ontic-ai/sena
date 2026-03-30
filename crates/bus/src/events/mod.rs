//! Typed event definitions.
//!
//! This is the single source of truth for all event types in Sena.
//! Events are organized by domain. All events are Clone + Send + 'static
//! and carry no logic — they are pure data.

pub mod ctp;
pub mod inference;
pub mod memory;
pub mod platform;
pub mod soul;
pub mod system;
pub mod transparency;

pub use ctp::*;
pub use inference::*;
pub use memory::*;
pub use platform::*;
pub use soul::*;
pub use system::*;
pub use transparency::*;
