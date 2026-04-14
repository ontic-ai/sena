//! Actor trait and error types.

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
pub trait Actor: Send + 'static {
    /// Actor's unique name for logging and identification.
    fn name(&self) -> &'static str;

    /// Initialize actor with bus access. Called once before `run()`.
    fn start(
        &mut self,
        bus: Arc<EventBus>,
    ) -> impl std::future::Future<Output = Result<(), ActorError>> + Send;

    /// Main event loop. Blocks until shutdown signal received.
    fn run(&mut self) -> impl std::future::Future<Output = Result<(), ActorError>> + Send;

    /// Graceful shutdown. Called after `run()` completes.
    fn stop(&mut self) -> impl std::future::Future<Output = Result<(), ActorError>> + Send;
}
