//! Platform adapter layer — OS-agnostic interface to platform signals.
//!
//! This crate provides:
//! - `PlatformBackend` trait: contract for OS-specific implementations
//! - `PlatformActor`: actor that owns a backend and processes signal requests
//! - `PlatformSignal`: unified signal type returned by backend methods
//! - Privacy-safe types: WindowContext, ClipboardDigest, KeystrokeCadence, ScreenFrame
//!
//! ## Architecture
//!
//! The platform crate sits between the runtime and OS-specific code:
//! - Defines the `PlatformBackend` trait with one method per signal type
//! - Re-exports privacy-safe types from `bus::events`
//! - Provides a stub actor for development and testing
//!
//! ## Privacy Boundary
//!
//! `KeystrokeCadence` is a compile-time privacy boundary. It must never contain
//! char, String, Vec<char>, or Vec<u8> fields that could represent character content.
//!
//! ## Dependencies
//!
//! Allowed: bus, thiserror, tracing, tokio, serde  
//! Forbidden: soul, memory, inference, OS-specific external crates (in this BONES unit)

pub mod actor;
pub mod backend;
pub mod error;
pub mod types;

pub use actor::PlatformActor;
pub use backend::PlatformBackend;
pub use error::PlatformError;
pub use types::{
    ClipboardDigest, FileEvent, FileEventKind, KeystrokeCadence, PlatformSignal, ScreenFrame,
    WindowContext,
};
