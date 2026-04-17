//! Platform adapter layer — OS-agnostic interface to platform signals.
//!
//! This crate provides:
//! - `PlatformBackend` trait: contract for OS-specific implementations
//! - `PlatformActor`: actor that owns a backend and processes signal requests
//! - `PlatformAdapter`: high-level interface with subscribe methods
//! - `PlatformSignal`: unified signal type returned by backend methods
//! - Privacy-safe types: WindowContext, ClipboardDigest, KeystrokeCadence, ScreenFrame
//!
//! ## Architecture
//!
//! The platform crate sits between the runtime and OS-specific code:
//! - Defines the `PlatformBackend` trait with one method per signal type
//! - Re-exports privacy-safe types from `bus::events`
//! - Provides OS-specific implementations for Windows, macOS, and Linux
//!
//! ## Privacy Boundary
//!
//! `KeystrokeCadence` is a compile-time privacy boundary. It must never contain
//! char, String, Vec<char>, or Vec<u8> fields that could represent character content.
//!
//! ## Dependencies
//!
//! Allowed: bus, thiserror, tracing, tokio, serde, arboard, notify, rdev, sha2
//! OS-specific: windows, core-graphics, core-foundation, x11rb

pub mod actor;
pub mod adapter;
pub mod backend;
pub mod backends;
pub mod error;
pub mod monitor;
pub mod types;

pub use actor::PlatformActor;
pub use adapter::PlatformAdapter;
pub use backend::PlatformBackend;
pub use backends::NativeBackend;
pub use error::PlatformError;
pub use types::{
    ClipboardDigest, FileEvent, FileEventKind, KeystrokeCadence, PlatformSignal, ScreenFrame,
    WindowContext,
};
