//! IPC Protocol for Sena Daemon-CLI Communication
//!
//! This crate provides the wire protocol, command dispatch mechanism, and client/server
//! implementations for Sena's inter-process communication. It is a leaf crate with no
//! dependencies on other Sena workspace crates.
//!
//! # Architecture
//!
//! - **Wire protocol**: 4-byte little-endian length-prefixed UTF-8 JSON frames
//! - **Transport**: Windows Named Pipe (`\\.\pipe\sena`)
//! - **Command dispatch**: Generic `CommandHandler` trait with `CommandRegistry`
//! - **Concurrency**: Server accepts multiple concurrent clients, each in own task
//! - **Push events**: Server can send unsolicited `IpcResponse` frames to subscribed clients

mod client;
mod error;
mod framing;
mod handler;
mod protocol;
mod registry;
mod server;

pub use client::IpcClient;
pub use error::IpcError;
pub use framing::{read_frame, write_frame};
pub use handler::CommandHandler;
pub use protocol::{IpcRequest, IpcResponse};
pub use registry::CommandRegistry;
pub use server::IpcServer;

/// Sena daemon named pipe identifier.
///
/// This is the single source of truth for the pipe name used by both daemon and CLI.
pub const PIPE_NAME: &str = r"\\.\pipe\sena";
