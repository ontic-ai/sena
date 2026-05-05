//! Actor trait and error types.

use async_trait::async_trait;
use std::sync::Arc;

use crate::bus::EventBus;

/// Actor lifecycle errors.
#[derive(Debug, thiserror::Error)]
pub enum ActorError {
    /// Channel closed during operation.
    #[error("channel closed: {0}")]
    ChannelClosed(String),

    /// Actor failed to start.
    #[error("startup failed: {0}")]
    StartupFailed(String),

    /// Runtime error during actor execution.
    #[error("runtime error: {0}")]
    RuntimeError(String),
}

/// Actor trait defining lifecycle: start → run → stop.
///
/// Actors are isolated units that communicate via the event bus.
/// Each actor follows a three-phase lifecycle:
///
/// 1. `start()`: Initialize resources, subscribe to bus channels
/// 2. `run()`: Main event loop, blocking until shutdown signal
/// 3. `stop()`: Clean shutdown, close channels, release resources
#[async_trait]
pub trait Actor: Send + 'static {
    /// Actor's unique name for logging and identification.
    fn name(&self) -> &'static str;

    /// Initialize actor with bus access. Called once before `run()`.
    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError>;

    /// Main event loop. Blocks until shutdown signal received.
    async fn run(&mut self) -> Result<(), ActorError>;

    /// Graceful shutdown. Called after `run()` completes.
    async fn stop(&mut self) -> Result<(), ActorError>;
}
