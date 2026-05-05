//! Wakeword detection actor.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};

use bus::events::{SpeechEvent, SystemEvent};
use bus::{Actor, ActorError, Event, EventBus};

use crate::audio_input::{AudioChunk, AudioInputConfig, AudioInputStream};

/// Wakeword detection configuration.
#[derive(Debug, Clone)]
pub struct WakewordConfig {
    /// Detection sensitivity [0.0, 1.0]. Higher = more sensitive.
    pub sensitivity: f32,
    /// Path to a specific wakeword model file.
    pub model_path: Option<PathBuf>,
    /// Directory where wakeword models are stored.
    pub model_dir: Option<PathBuf>,
    /// Minimum time between consecutive detections.
    pub debounce_secs: f32,
}

impl Default for WakewordConfig {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            model_path: None,
            model_dir: None,
            debounce_secs: 3.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WakewordBackend {
    Model,
    EnergyBased,
    Mock,
}

/// Wakeword detection actor.
pub struct WakewordActor {
    bus: Option<Arc<EventBus>>,
    bus_rx: Option<broadcast::Receiver<Event>>,
    sensitivity: f32,
    model_path: Option<PathBuf>,
    #[allow(dead_code)]
    model_dir: Option<PathBuf>,
    backend: WakewordBackend,
    audio_stream: Option<AudioInputStream>,
    audio_rx: Option<mpsc::UnboundedReceiver<AudioChunk>>,
    debounce_duration: Duration,
    last_detection: Option<Instant>,
    background_noise_level: f32,
    noise_samples_seen: u32,
    suppressed: bool,
    loop_enabled: bool,
}

impl WakewordActor {
    pub fn new(config: WakewordConfig) -> Self {
        let backend = if config.model_path.is_some() || config.model_dir.is_some() {
            WakewordBackend::Model
        } else {
            WakewordBackend::EnergyBased
        };

        Self {
            bus: None,
            bus_rx: None,
            sensitivity: config.sensitivity.clamp(0.0, 1.0),
            model_path: config.model_path,
            model_dir: config.model_dir,
            backend,
            audio_stream: None,
            audio_rx: None,
            debounce_duration: Duration::from_secs_f32(config.debounce_secs),
            last_detection: None,
            background_noise_level: 0.01,
            noise_samples_seen: 0,
            suppressed: false,
            loop_enabled: true,
        }
    }

    pub fn mock() -> Self {
        Self {
            bus: None,
            bus_rx: None,
            sensitivity: 0.5,
            model_path: None,
            model_dir: None,
            backend: WakewordBackend::Mock,
            audio_stream: None,
            audio_rx: None,
            debounce_duration: Duration::from_secs(3),
            last_detection: None,
            background_noise_level: 0.01,
            noise_samples_seen: 0,
            suppressed: false,
            loop_enabled: true,
        }
    }

    async fn initialize_backend(&mut self) -> Result<(), ActorError> {
        match self.backend {
            WakewordBackend::Model => {
                let model_exists = self
                    .model_path
                    .as_ref()
                    .map(|path| path.exists())
                    .unwrap_or(false);

                if !model_exists {
                    self.backend = WakewordBackend::EnergyBased;
                }

                Ok(())
            }
            WakewordBackend::EnergyBased | WakewordBackend::Mock => Ok(()),
        }
    }

    fn start_audio_capture(&mut self) -> Result<(), ActorError> {
        if matches!(self.backend, WakewordBackend::Mock) {
            return Ok(());
        }

        let config = AudioInputConfig {
            sample_rate: 16_000,
            buffer_duration_secs: 0.5,
        };

        let (stream, rx) = AudioInputStream::start(config)
            .map_err(|err| ActorError::StartupFailed(format!("audio capture failed: {err}")))?;

        self.audio_stream = Some(stream);
        self.audio_rx = Some(rx);
        Ok(())
    }

    fn stop_audio_capture(&mut self) {
        self.audio_stream = None;
        self.audio_rx = None;
    }

    fn should_detect(&self) -> bool {
        if let Some(last) = self.last_detection {
            last.elapsed() >= self.debounce_duration
        } else {
            true
        }
    }

    fn calculate_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }

        let sum_squares: f32 = samples.iter().map(|sample| sample * sample).sum();
        (sum_squares / samples.len() as f32).sqrt()
    }

    fn update_background_noise(&mut self, rms: f32) {
        const NOISE_ALPHA: f32 = 0.1;
        self.background_noise_level =
            NOISE_ALPHA * rms + (1.0 - NOISE_ALPHA) * self.background_noise_level;
        self.noise_samples_seen = self.noise_samples_seen.saturating_add(1);
    }

    fn detect_energy_based(&mut self, chunk: &AudioChunk) -> Option<f32> {
        let rms = Self::calculate_rms(&chunk.samples);

        if self.noise_samples_seen < 10 {
            self.update_background_noise(rms);
            return None;
        }

        let threshold_multiplier = 2.0 + (1.0 - self.sensitivity) * 3.0;
        let threshold = self.background_noise_level * threshold_multiplier;

        if rms > threshold && self.should_detect() {
            let excess_ratio = (rms / threshold).min(3.0);
            let confidence = ((excess_ratio - 1.0) / 2.0).clamp(0.0, 1.0);
            Some(confidence)
        } else {
            if rms < threshold * 0.8 {
                self.update_background_noise(rms);
            }
            None
        }
    }

    async fn process_audio_chunk(
        &mut self,
        chunk: AudioChunk,
        bus: &Arc<EventBus>,
    ) -> Result<(), ActorError> {
        if self.suppressed || !self.loop_enabled {
            return Ok(());
        }

        let confidence = match self.backend {
            WakewordBackend::EnergyBased | WakewordBackend::Model => {
                self.detect_energy_based(&chunk)
            }
            WakewordBackend::Mock => None,
        };

        if let Some(confidence) = confidence {
            self.last_detection = Some(Instant::now());
            bus.broadcast(Event::Speech(SpeechEvent::WakewordDetected { confidence }))
                .await
                .map_err(|err| ActorError::RuntimeError(format!("broadcast failed: {err}")))?;
        }

        Ok(())
    }

    async fn handle_bus_event(
        &mut self,
        event: Event,
        bus: &Arc<EventBus>,
    ) -> Result<bool, ActorError> {
        match event {
            Event::System(SystemEvent::ShutdownSignal)
            | Event::System(SystemEvent::ShutdownRequested) => return Ok(true),
            Event::System(SystemEvent::LoopControlRequested { loop_name, enabled })
                if loop_name == "speech" =>
            {
                self.loop_enabled = enabled;
                if enabled {
                    if self.audio_stream.is_none() {
                        self.start_audio_capture()?;
                    }
                } else {
                    self.stop_audio_capture();
                }

                bus.broadcast(Event::System(SystemEvent::LoopStatusChanged {
                    loop_name: "speech".to_string(),
                    enabled,
                }))
                .await
                .map_err(|err| ActorError::RuntimeError(format!("broadcast failed: {err}")))?;
            }
            Event::Speech(SpeechEvent::SpeakingStarted { causal_id }) => {
                self.suppressed = true;
                bus.broadcast(Event::Speech(SpeechEvent::WakewordSuppressed {
                    reason: "TTS playing".to_string(),
                    causal_id,
                }))
                .await
                .map_err(|err| ActorError::RuntimeError(format!("broadcast failed: {err}")))?;
            }
            Event::Speech(
                SpeechEvent::SpeakingCompleted { causal_id }
                | SpeechEvent::SpeakingInterrupted { causal_id }
                | SpeechEvent::SpeechFailed { causal_id, .. },
            ) => {
                self.suppressed = false;
                bus.broadcast(Event::Speech(SpeechEvent::WakewordResumed { causal_id }))
                    .await
                    .map_err(|err| ActorError::RuntimeError(format!("broadcast failed: {err}")))?;
            }
            Event::Speech(SpeechEvent::ListenModeRequested { causal_id }) => {
                self.suppressed = true;
                self.stop_audio_capture();
                bus.broadcast(Event::Speech(SpeechEvent::WakewordSuppressed {
                    reason: "listen mode active".to_string(),
                    causal_id,
                }))
                .await
                .map_err(|err| ActorError::RuntimeError(format!("broadcast failed: {err}")))?;
            }
            Event::Speech(SpeechEvent::ListenModeStopped { causal_id, .. }) => {
                self.suppressed = false;
                if self.loop_enabled {
                    self.start_audio_capture()?;
                }
                bus.broadcast(Event::Speech(SpeechEvent::WakewordResumed { causal_id }))
                    .await
                    .map_err(|err| ActorError::RuntimeError(format!("broadcast failed: {err}")))?;
            }
            _ => {}
        }

        Ok(false)
    }
}

impl Actor for WakewordActor {
    fn name(&self) -> &'static str {
        "wakeword"
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        let bus_rx = bus.subscribe_broadcast();
        self.bus = Some(bus);
        self.bus_rx = Some(bus_rx);

        self.initialize_backend().await?;
        if self.loop_enabled {
            self.start_audio_capture()?;
        }

        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let mut bus_rx = self
            .bus_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("bus_rx not initialized".to_string()))?;

        let bus = Arc::clone(
            self.bus
                .as_ref()
                .ok_or_else(|| ActorError::RuntimeError("bus not initialized".to_string()))?,
        );

        loop {
            tokio::select! {
                audio_chunk = async {
                    if let Some(rx) = self.audio_rx.as_mut() {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    if let Some(chunk) = audio_chunk {
                        self.process_audio_chunk(chunk, &bus).await?;
                    }
                }

                bus_event = bus_rx.recv() => {
                    match bus_event {
                        Ok(event) => {
                            if self.handle_bus_event(event, &bus).await? {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(ActorError::ChannelClosed("bus_rx closed".to_string()));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        self.stop_audio_capture();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakeword_config_defaults() {
        let config = WakewordConfig::default();
        assert_eq!(config.sensitivity, 0.5);
        assert_eq!(config.debounce_secs, 3.0);
        assert!(config.model_path.is_none());
        assert!(config.model_dir.is_none());
    }

    #[test]
    fn wakeword_actor_mock_creation() {
        let actor = WakewordActor::mock();
        assert_eq!(actor.backend, WakewordBackend::Mock);
        assert_eq!(actor.sensitivity, 0.5);
    }

    #[test]
    fn wakeword_actor_new_defaults_to_energy_based() {
        let actor = WakewordActor::new(WakewordConfig::default());
        assert_eq!(actor.backend, WakewordBackend::EnergyBased);
    }

    #[test]
    fn calculate_rms_empty_returns_zero() {
        let rms = WakewordActor::calculate_rms(&[]);
        assert_eq!(rms, 0.0);
    }

    #[test]
    fn detect_energy_based_high_energy_triggers_detection() {
        let mut actor = WakewordActor::new(WakewordConfig::default());

        for _ in 0..20 {
            actor.detect_energy_based(&AudioChunk {
                samples: vec![0.01; 100],
            });
        }

        let result = actor.detect_energy_based(&AudioChunk {
            samples: vec![0.5; 100],
        });
        assert!(result.is_some());
        if let Some(confidence) = result {
            assert!(confidence > 0.0);
            assert!(confidence <= 1.0);
        }
    }

    #[test]
    fn debounce_prevents_rapid_detections() {
        let mut actor = WakewordActor::new(WakewordConfig {
            debounce_secs: 1.0,
            ..Default::default()
        });

        assert!(actor.should_detect());
        actor.last_detection = Some(Instant::now());
        assert!(!actor.should_detect());
        actor.last_detection = Some(Instant::now() - Duration::from_secs(2));
        assert!(actor.should_detect());
    }
}
