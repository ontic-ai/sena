//! Prompt actor — bus-event-driven orchestrator for prompt assembly.
//!
//! `PromptActor` subscribes to the event bus and assembles typed prompts
//! using the configured `PromptComposer` when inference is requested.
//!
//! ## BONES status
//!
//! This is a stub implementation. The actor starts, emits `ActorReady`,
//! then listens for shutdown events. Real implementations will intercept
//! `InferenceRequested` events, assemble context via `PromptComposer`,
//! and rebroadcast an enriched prompt before it reaches the InferenceActor.

use crate::composer::PromptComposer;
use bus::{Actor, ActorError, Event, EventBus, SystemEvent};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Configuration for the prompt actor.
#[derive(Debug, Clone, Default)]
pub struct PromptConfig {
    /// Token budget applied during prompt assembly.
    ///
    /// If None, no token limit is enforced.
    pub token_limit: Option<usize>,
}

/// Prompt actor — owns a composer and assembles prompts on bus events.
pub struct PromptActor {
    #[allow(dead_code)]
    composer: Box<dyn PromptComposer>,
    config: PromptConfig,
    rx: Option<broadcast::Receiver<Event>>,
}

impl PromptActor {
    /// Create a new prompt actor with the given composer and default config.
    pub fn new(composer: Box<dyn PromptComposer>) -> Self {
        Self {
            composer,
            config: PromptConfig::default(),
            rx: None,
        }
    }

    /// Create a new prompt actor with explicit configuration.
    pub fn with_config(composer: Box<dyn PromptComposer>, config: PromptConfig) -> Self {
        Self {
            composer,
            config,
            rx: None,
        }
    }
}

impl Actor for PromptActor {
    fn name(&self) -> &'static str {
        "prompt"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!(
            actor = self.name(),
            token_limit = ?self.config.token_limit,
            "PromptActor starting"
        );
        self.rx = Some(bus.subscribe_broadcast());
        debug!(actor = self.name(), "PromptActor subscribed to bus");
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let rx = self.rx.as_mut().ok_or_else(|| {
            ActorError::StartupFailed("rx not initialized — call start() first".to_string())
        })?;

        let name = "prompt";
        info!(actor = name, "PromptActor running");

        loop {
            match rx.recv().await {
                Ok(Event::System(SystemEvent::ShutdownSignal))
                | Ok(Event::System(SystemEvent::ShutdownRequested))
                | Ok(Event::System(SystemEvent::ShutdownInitiated)) => {
                    info!(actor = name, "shutdown signal received — exiting");
                    break;
                }
                Ok(_event) => {
                    // BONES stub: log but do not process events yet.
                    // Real implementation: intercept InferenceRequested, assemble context,
                    // rebroadcast enriched prompt.
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(actor = name, lagged = n, "broadcast channel lagged");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!(actor = name, "broadcast channel closed");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!(actor = self.name(), "PromptActor stopped");
        self.rx = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::StubComposer;

    #[test]
    fn prompt_actor_constructs() {
        let composer = Box::new(StubComposer::default_segments());
        let actor = PromptActor::new(composer);
        assert_eq!(actor.name(), "prompt");
    }

    #[test]
    fn prompt_actor_with_config() {
        let composer = Box::new(StubComposer::default_segments());
        let config = PromptConfig {
            token_limit: Some(4096),
        };
        let actor = PromptActor::with_config(composer, config);
        assert_eq!(actor.config.token_limit, Some(4096));
    }
}
