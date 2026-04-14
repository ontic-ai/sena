//! Inference actor — receives InferenceRequested events and produces token streams.

use crate::backend::InferenceBackend;
use crate::error::InferenceError;
use crate::types::InferenceParams;
use bus::{Actor, ActorError, Event, EventBus, InferenceEvent};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

/// Inference actor.
pub struct InferenceActor {
    bus: Option<Arc<EventBus>>,
    backend: Arc<Mutex<Box<dyn InferenceBackend>>>,
    rx: Option<broadcast::Receiver<Event>>,
}

impl InferenceActor {
    /// Create a new inference actor with the given backend.
    pub fn new(backend: Box<dyn InferenceBackend>) -> Self {
        Self {
            bus: None,
            backend: Arc::new(Mutex::new(backend)),
            rx: None,
        }
    }

    /// Handle an inference request event.
    async fn handle_inference_request(
        &self,
        prompt: String,
        source: bus::InferenceSource,
        priority: bus::Priority,
        causal_id: bus::CausalId,
    ) -> Result<(), InferenceError> {
        info!(
            ?source,
            ?priority,
            ?causal_id,
            prompt_len = prompt.len(),
            "inference request received"
        );

        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| InferenceError::ActorError("bus not initialized".to_string()))?;

        // Lock the backend
        let backend = self.backend.lock().await;

        if !backend.is_loaded() {
            warn!("inference requested but no model loaded");
            bus.broadcast(Event::Inference(InferenceEvent::InferenceFailed {
                reason: "no model loaded".to_string(),
                causal_id,
            }))
            .await?;
            return Err(InferenceError::ModelNotLoaded);
        }

        // Create inference parameters (stub: use defaults)
        let params = InferenceParams::default();

        debug!(?params, "running inference with default params");

        // Run inference and get stream
        let mut stream = backend.infer(prompt.clone(), params).await?;

        // Collect tokens and emit token events
        let mut token_count: u64 = 0;
        let mut full_text = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(token) => {
                    full_text.push_str(&token);
                    token_count += 1;

                    bus.broadcast(Event::Inference(InferenceEvent::InferenceTokenGenerated {
                        token,
                        sequence_number: token_count,
                        causal_id,
                    }))
                    .await?;
                }
                Err(e) => {
                    warn!(error = ?e, "inference stream error");
                    bus.broadcast(Event::Inference(InferenceEvent::InferenceFailed {
                        reason: e.to_string(),
                        causal_id,
                    }))
                    .await?;
                    return Err(e);
                }
            }
        }

        // Emit completion event
        info!(
            token_count,
            text_len = full_text.len(),
            "inference complete"
        );
        bus.broadcast(Event::Inference(InferenceEvent::InferenceCompleted {
            text: full_text,
            token_count: token_count as usize,
            causal_id,
        }))
        .await?;

        Ok(())
    }
}

impl Actor for InferenceActor {
    fn name(&self) -> &'static str {
        "inference"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        info!("inference actor starting");
        self.rx = Some(bus.subscribe_broadcast());
        self.bus = Some(bus);
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        info!("inference actor running");

        let mut rx = self
            .rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("receiver not initialized".to_string()))?;

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Event::Inference(InferenceEvent::InferenceRequested {
                        prompt,
                        priority,
                        source,
                        causal_id,
                    }) = event
                    {
                        if let Err(e) = self
                            .handle_inference_request(prompt, source, priority, causal_id)
                            .await
                        {
                            warn!(error = ?e, "inference request handling failed");
                        }
                    }
                }
                Err(e) => {
                    warn!(error = ?e, "bus recv error");
                    return Err(ActorError::ChannelClosed(e.to_string()));
                }
            }
        }
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        info!("inference actor stopping");
        let mut backend = self.backend.lock().await;
        backend
            .shutdown()
            .await
            .map_err(|e| ActorError::RuntimeError(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::InferenceBackend;
    use crate::stream::InferenceStream;
    use crate::types::BackendType;

    /// Mock backend for testing.
    struct MockBackend {
        loaded: bool,
    }

    #[async_trait::async_trait]
    impl InferenceBackend for MockBackend {
        fn backend_type(&self) -> BackendType {
            BackendType::Mock
        }

        fn is_loaded(&self) -> bool {
            self.loaded
        }

        async fn infer(
            &self,
            _prompt: String,
            _params: InferenceParams,
        ) -> Result<InferenceStream, InferenceError> {
            let (tx, stream) = InferenceStream::channel(10);
            tokio::spawn(async move {
                let _ = tx.send(Ok("mock".to_string())).await;
                let _ = tx.send(Ok(" response".to_string())).await;
            });
            Ok(stream)
        }

        async fn shutdown(&mut self) -> Result<(), InferenceError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn actor_handles_inference_request_when_model_loaded() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend { loaded: true });
        let mut actor = InferenceActor::new(backend);

        let mut rx = bus.subscribe_broadcast();

        // Start and run actor
        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Send inference request
        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test".to_string(),
            priority: bus::Priority::Normal,
            source: bus::InferenceSource::UserText,
            causal_id,
        }))
        .await
        .unwrap();

        // Collect events
        let mut completed = false;
        let mut token_count = 0;

        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let Ok(Event::Inference(inf_event)) = event {
                    match inf_event {
                        InferenceEvent::InferenceTokenGenerated { .. } => {
                            token_count += 1;
                        }
                        InferenceEvent::InferenceCompleted { .. } => {
                            completed = true;
                            break;
                        }
                        _ => {}
                    }
                }
            } else {
                break;
            }
        }

        assert!(completed, "should receive completion event");
        assert_eq!(token_count, 2, "should receive 2 token events");
    }

    #[tokio::test]
    async fn actor_emits_failure_when_model_not_loaded() {
        let bus = Arc::new(EventBus::new());
        let backend = Box::new(MockBackend { loaded: false });
        let mut actor = InferenceActor::new(backend);

        let mut rx = bus.subscribe_broadcast();

        // Start and run actor
        actor.start(bus.clone()).await.unwrap();
        tokio::spawn(async move {
            let _ = actor.run().await;
        });

        // Send inference request
        let causal_id = bus::CausalId::new();
        bus.broadcast(Event::Inference(InferenceEvent::InferenceRequested {
            prompt: "test".to_string(),
            priority: bus::Priority::Normal,
            source: bus::InferenceSource::UserText,
            causal_id,
        }))
        .await
        .unwrap();

        // Expect failure event
        let mut failed = false;
        for _ in 0..10 {
            if let Ok(event) =
                tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
            {
                if let Ok(Event::Inference(InferenceEvent::InferenceFailed { reason, .. })) = event
                {
                    assert_eq!(reason, "no model loaded");
                    failed = true;
                    break;
                }
            } else {
                break;
            }
        }

        assert!(failed, "should receive failure event");
    }
}
