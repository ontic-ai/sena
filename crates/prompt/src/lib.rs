//! Dynamic prompt composition â€” zero static strings.
//!
//! All prompt content flows through typed [`PromptSegment`] variants.
//! [`PromptComposer`] is stateless; construct one per inference cycle.

pub mod composer;
pub mod error;
pub mod segment;

pub use composer::PromptComposer;
pub use error::PromptError;
pub use segment::{PromptSegment, ReflectionMode};
