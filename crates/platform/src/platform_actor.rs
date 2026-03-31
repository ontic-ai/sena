//! Platform actor: polls OS signals and emits events on the bus.

use async_trait::async_trait;
use bus::events::platform::{ClipboardDigest, WindowContext};
use bus::{Actor, ActorError, Event, EventBus, PlatformEvent, SystemEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::adapter::PlatformAdapter;

/// Platform actor polls the platform adapter and emits events on the bus.
pub struct PlatformActor {
    adapter: Box<dyn PlatformAdapter>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    bus: Option<Arc<EventBus>>,
    poll_interval: Duration,
    last_window: Option<WindowContext>,
    last_clipboard: Option<ClipboardDigest>,
}

impl PlatformActor {
    /// Create a new platform actor with the given adapter.
    pub fn new(adapter: Box<dyn PlatformAdapter>) -> Self {
        Self {
            adapter,
            bus_rx: None,
            bus: None,
            poll_interval: Duration::from_millis(500),
            last_window: None,
            last_clipboard: None,
        }
    }

    /// Set the polling interval for checking platform signals.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Check for window changes and emit event if changed.
    async fn check_window_change(&mut self) -> Result<(), ActorError> {
        if let Some(current) = self.adapter.active_window() {
            let should_emit = self
                .last_window
                .as_ref()
                .map(|last| last.app_name != current.app_name)
                .unwrap_or(true);

            if should_emit {
                if let Some(bus) = &self.bus {
                    bus.broadcast(Event::Platform(PlatformEvent::WindowChanged(
                        current.clone(),
                    )))
                    .await
                    .map_err(|e| ActorError::RuntimeError(format!("broadcast failed: {}", e)))?;
                }
                self.last_window = Some(current);
            }
        }
        Ok(())
    }

    /// Check for clipboard changes and emit event if changed.
    async fn check_clipboard_change(&mut self) -> Result<(), ActorError> {
        if let Some(current) = self.adapter.clipboard_digest() {
            let should_emit = self
                .last_clipboard
                .as_ref()
                .map(|last| last.digest != current.digest)
                .unwrap_or(true);

            if should_emit {
                if let Some(bus) = &self.bus {
                    bus.broadcast(Event::Platform(PlatformEvent::ClipboardChanged(
                        current.clone(),
                    )))
                    .await
                    .map_err(|e| ActorError::RuntimeError(format!("broadcast failed: {}", e)))?;
                }
                self.last_clipboard = Some(current);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Actor for PlatformActor {
    fn name(&self) -> &'static str {
        "platform"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        self.bus_rx = Some(bus.subscribe_broadcast());
        self.bus = Some(bus.clone());

        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: "Platform",
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e)))?;

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut interval = tokio::time::interval(self.poll_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.check_window_change().await?;
                    self.check_clipboard_change().await?;
                }
                event = async {
                    match &mut self.bus_rx {
                        Some(rx) => rx.recv().await,
                        None => Err(broadcast::error::RecvError::Closed),
                    }
                } => {
                    match event {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                            return Ok(());
                        }
                        Err(_) => {
                            return Err(ActorError::ChannelClosed("bus channel closed".to_string()));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        self.bus_rx = None;
        self.bus = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::create_platform_adapter;

    #[test]
    fn platform_actor_implements_actor_trait() {
        let adapter = create_platform_adapter();
        let actor = PlatformActor::new(adapter);
        assert_eq!(actor.name(), "platform");
    }

    #[tokio::test]
    async fn platform_actor_starts_and_stops() {
        let adapter = create_platform_adapter();
        let mut actor = PlatformActor::new(adapter);

        let bus = Arc::new(EventBus::new());
        actor.start(bus).await.expect("start should succeed");

        actor.stop().await.expect("stop should succeed");
        assert!(actor.bus_rx.is_none());
        assert!(actor.bus.is_none());
    }

    #[tokio::test]
    async fn platform_actor_stops_on_shutdown_signal() {
        let adapter = create_platform_adapter();
        let mut actor = PlatformActor::new(adapter).with_poll_interval(Duration::from_millis(100));

        let bus = Arc::new(EventBus::new());
        actor
            .start(bus.clone())
            .await
            .expect("start should succeed");

        // Spawn the run loop
        let run_handle = tokio::spawn(async move { actor.run().await });

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send shutdown signal
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        // Run loop should exit cleanly
        let result = tokio::time::timeout(Duration::from_secs(1), run_handle).await;
        assert!(result.is_ok(), "actor should stop within timeout");
        assert!(result.unwrap().is_ok(), "run should return Ok");
    }
}
