//! Boot sequence implementation.

use crate::builder;
use crate::error::RuntimeError;
use bus::{Actor, Event, EventBus, SystemEvent};
use crypto::EncryptionLayer;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Threshold for boot step timing warnings (500ms).
const BOOT_STEP_WARN_THRESHOLD_MS: u128 = 500;

/// Boot result containing all runtime components.
pub struct BootResult {
    /// Event bus shared by all actors.
    pub bus: Arc<EventBus>,
    /// Encryption layer for encrypted storage.
    pub encryption: Arc<dyn EncryptionLayer>,
    /// Spawned actor handles with their names.
    pub actor_handles: Vec<(&'static str, JoinHandle<()>)>,
    /// List of actor names that should emit ActorReady before BootComplete.
    pub expected_actors: Vec<&'static str>,
    /// Pre-subscribed broadcast receiver for readiness gate.
    /// Subscribed BEFORE actors are spawned to avoid missing early ActorReady events.
    /// Should be taken (consumed) by the supervisor's readiness gate.
    pub readiness_rx: Option<tokio::sync::broadcast::Receiver<Event>>,
}

/// Execute the boot sequence.
///
/// Boot order:
/// 1. Config load (stub)
/// 2. Encryption init
/// 3. EventBus init
/// 4. Soul actor spawn
/// 5. Inference actor spawn
/// 6. Memory actor spawn
/// 7. Platform adapter spawn
/// 8. CTP actor spawn
/// 9. Prompt actor spawn
/// 10. Speech STT actor spawn
/// 11. Speech TTS actor spawn
/// 12. Wait for all ActorReady events (handled by supervisor)
/// 13. Emit BootComplete (handled by supervisor)
///
/// Returns BootResult on success, RuntimeError on failure.
pub async fn boot() -> Result<BootResult, RuntimeError> {
    let boot_start = Instant::now();
    info!("BOOT START: Sena runtime initializing");

    // Step 1: Config load
    let step_start = Instant::now();
    info!("Step 1/5: Loading configuration");
    load_config().await?;
    check_step_timing("config load", step_start);

    // Step 2: Encryption init
    let step_start = Instant::now();
    info!("Step 2/5: Initializing encryption layer");
    let encryption = init_encryption().await?;
    check_step_timing("encryption init", step_start);

    // Step 3: EventBus init
    let step_start = Instant::now();
    info!("Step 3/5: Initializing event bus");
    let bus = Arc::new(EventBus::new());
    check_step_timing("event bus init", step_start);

    // Subscribe to broadcast BEFORE spawning actors to avoid missing early ActorReady events.
    // This receiver will be used by the supervisor's readiness gate.
    let readiness_rx = bus.subscribe_broadcast();

    // Broadcast EncryptionInitialized event (stub: ignore if no subscribers)
    let _ = bus
        .broadcast(Event::System(SystemEvent::EncryptionInitialized))
        .await;

    // Step 4: Soul init
    let step_start = Instant::now();
    info!("Step 4/5: Initializing Soul subsystem");
    init_soul(bus.clone(), encryption.clone()).await?;
    check_step_timing("soul init", step_start);

    // Step 5: Core actors spawn
    let step_start = Instant::now();
    info!("Step 5/5: Spawning core actors");
    let (actor_handles, expected_actors) = spawn_actors(bus.clone()).await?;
    check_step_timing("actor spawn", step_start);

    let boot_elapsed = boot_start.elapsed();
    info!(
        actor_count = actor_handles.len(),
        boot_time_ms = boot_elapsed.as_millis(),
        "BOOT SEQUENCE COMPLETE: All actors spawned (waiting for readiness)"
    );

    Ok(BootResult {
        bus,
        encryption,
        actor_handles,
        expected_actors,
        readiness_rx: Some(readiness_rx),
    })
}

/// Check if a boot step exceeded the warning threshold and log if so.
fn check_step_timing(step_name: &str, start: Instant) {
    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis();

    if elapsed_ms > BOOT_STEP_WARN_THRESHOLD_MS {
        warn!(
            step = step_name,
            elapsed_ms = elapsed_ms,
            "Boot step exceeded 500ms threshold"
        );
    } else {
        info!(
            step = step_name,
            elapsed_ms = elapsed_ms,
            "Boot step completed"
        );
    }
}

/// Load configuration from disk or create defaults.
async fn load_config() -> Result<(), RuntimeError> {
    // Stub implementation: configuration loading will be implemented in a later phase.
    tracing::debug!("config load: using default configuration (stub)");
    Ok(())
}

/// Initialize the encryption layer.
async fn init_encryption() -> Result<Arc<dyn EncryptionLayer>, RuntimeError> {
    // Stub implementation: use stub encryption layer.
    tracing::debug!("encryption init: using stub encryption layer");
    let layer: Arc<dyn EncryptionLayer> = Arc::new(crypto::StubEncryptionLayer);
    Ok(layer)
}

/// Initialize the Soul subsystem.
async fn init_soul(
    _bus: Arc<EventBus>,
    _encryption: Arc<dyn EncryptionLayer>,
) -> Result<(), RuntimeError> {
    // Stub implementation: Soul initialization will construct the encrypted store.
    tracing::debug!("soul init: schema loaded (stub)");
    Ok(())
}

/// Spawn all core actors.
///
/// Order matches the boot sequence:
/// 4. Soul
/// 5. Inference
/// 6. Memory
/// 7. Platform
/// 8. CTP
/// 9. Prompt
/// 10. STT
/// 11. TTS
///
/// Returns (actor_handles, expected_actors) where expected_actors is the list
/// of actor names that must emit ActorReady before BootComplete.
async fn spawn_actors(
    bus: std::sync::Arc<EventBus>,
) -> Result<
    (
        Vec<(&'static str, tokio::task::JoinHandle<()>)>,
        Vec<&'static str>,
    ),
    RuntimeError,
> {
    let mut handles = Vec::new();
    let mut expected = Vec::new();

    // Step 4: Soul actor spawn
    let soul_actor = builder::build_soul_actor()?;
    let soul_name: &'static str = "soul";
    expected.push(soul_name);
    let soul_handle = spawn_soul_actor(soul_actor, bus.clone());
    handles.push((soul_name, soul_handle));

    // Step 5: Inference actor spawn
    let inference_actor = builder::build_inference_actor()?;
    let inference_name: &'static str = "inference";
    expected.push(inference_name);
    let inference_handle = spawn_inference_actor(inference_actor, bus.clone());
    handles.push((inference_name, inference_handle));

    // Step 6: Memory actor spawn
    let memory_actor = builder::build_memory_actor()?;
    let memory_name: &'static str = "memory";
    expected.push(memory_name);
    let memory_handle = spawn_memory_actor(memory_actor, bus.clone());
    handles.push((memory_name, memory_handle));

    // Step 7: Platform adapter spawn
    let platform_actor = builder::build_platform_actor()?;
    let platform_name: &'static str = "platform";
    expected.push(platform_name);
    let platform_handle = spawn_platform_actor(platform_actor, bus.clone());
    handles.push((platform_name, platform_handle));

    // Step 8: CTP actor spawn
    // _ctp_signal_tx: kept alive for session duration so the channel endpoint
    // does not close prematurely. CTP uses the bus path in production; this
    // sender is available for future direct signal injection if needed.
    let (ctp_actor, _ctp_signal_tx) = builder::build_ctp_actor()?;
    let ctp_name: &'static str = "ctp";
    expected.push(ctp_name);
    let ctp_handle = spawn_ctp_actor(ctp_actor, bus.clone());
    handles.push((ctp_name, ctp_handle));

    // Step 9: Prompt actor spawn
    let prompt_actor = builder::build_prompt_actor()?;
    let prompt_name: &'static str = "prompt";
    expected.push(prompt_name);
    let prompt_handle = spawn_prompt_actor(prompt_actor, bus.clone());
    handles.push((prompt_name, prompt_handle));

    // Speech actors are spawned conditionally (stub: always spawn for now)
    let speech_enabled = true;
    if speech_enabled {
        // Step 10: STT actor spawn
        let stt_actor = builder::build_stt_actor()?;
        let stt_name: &'static str = "stt";
        expected.push(stt_name);
        let stt_handle = spawn_stt_actor(stt_actor, bus.clone());
        handles.push((stt_name, stt_handle));

        // Step 11: TTS actor spawn
        let tts_actor = builder::build_tts_actor()?;
        let tts_name: &'static str = "tts";
        expected.push(tts_name);
        let tts_handle = spawn_tts_actor(tts_actor, bus.clone());
        handles.push((tts_name, tts_handle));
    }

    Ok((handles, expected))
}

/// Spawn soul actor in a tokio task.
fn spawn_soul_actor(
    mut actor: soul::SoulActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = actor.name();
        tracing::info!(actor = name, "Actor task started");

        if let Err(e) = actor.start(bus.clone()).await {
            tracing::error!(actor = name, error = %e, "Actor start failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
            return;
        }

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        if let Err(e) = actor.run().await {
            tracing::error!(actor = name, error = %e, "Actor run failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
        }

        if let Err(e) = actor.stop().await {
            tracing::warn!(actor = name, error = %e, "Actor stop failed");
        }

        tracing::info!(actor = name, "Actor task stopped");
    })
}

/// Spawn memory actor in a tokio task.
fn spawn_memory_actor(
    mut actor: memory::MemoryActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = actor.name();
        tracing::info!(actor = name, "Actor task started");

        if let Err(e) = actor.start(bus.clone()).await {
            tracing::error!(actor = name, error = %e, "Actor start failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
            return;
        }

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        if let Err(e) = actor.run().await {
            tracing::error!(actor = name, error = %e, "Actor run failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
        }

        if let Err(e) = actor.stop().await {
            tracing::warn!(actor = name, error = %e, "Actor stop failed");
        }

        tracing::info!(actor = name, "Actor task stopped");
    })
}

/// Spawn inference actor in a tokio task.
fn spawn_inference_actor(
    mut actor: inference::InferenceActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = actor.name();
        tracing::info!(actor = name, "Actor task started");

        if let Err(e) = actor.start(bus.clone()).await {
            tracing::error!(actor = name, error = %e, "Actor start failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
            return;
        }

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        if let Err(e) = actor.run().await {
            tracing::error!(actor = name, error = %e, "Actor run failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
        }

        if let Err(e) = actor.stop().await {
            tracing::warn!(actor = name, error = %e, "Actor stop failed");
        }

        tracing::info!(actor = name, "Actor task stopped");
    })
}

/// Intervals for platform signal polling.
mod poll_intervals {
    use std::time::Duration;
    /// Active window polling — 250 ms.
    pub const WINDOW: Duration = Duration::from_millis(250);
    /// Clipboard polling — 500 ms.
    pub const CLIPBOARD: Duration = Duration::from_millis(500);
    /// Keystroke cadence polling — 100 ms.
    pub const KEYSTROKE: Duration = Duration::from_millis(100);
}

/// Spawn platform actor in a tokio task.
///
/// Calls the actor's run_polling_loop which polls the native backend at
/// configured intervals and broadcasts PlatformEvent on the EventBus.
fn spawn_platform_actor(
    actor: platform::PlatformActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = "platform";
        tracing::info!(actor = name, "Platform actor started");

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        actor
            .run_polling_loop(
                bus,
                poll_intervals::WINDOW,
                poll_intervals::CLIPBOARD,
                poll_intervals::KEYSTROKE,
            )
            .await;

        tracing::info!(actor = name, "Platform actor stopped");
    })
}

/// Spawn CTP actor in a tokio task.
fn spawn_ctp_actor(
    mut actor: ctp::CtpActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = actor.name();
        tracing::info!(actor = name, "Actor task started");

        if let Err(e) = actor.start(bus.clone()).await {
            tracing::error!(actor = name, error = %e, "Actor start failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
            return;
        }

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        if let Err(e) = actor.run().await {
            tracing::error!(actor = name, error = %e, "Actor run failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
        }

        if let Err(e) = actor.stop().await {
            tracing::warn!(actor = name, error = %e, "Actor stop failed");
        }

        tracing::info!(actor = name, "Actor task stopped");
    })
}

/// Spawn prompt actor in a tokio task.
fn spawn_prompt_actor(
    mut actor: prompt::PromptActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = actor.name();
        tracing::info!(actor = name, "Actor task started");

        if let Err(e) = actor.start(bus.clone()).await {
            tracing::error!(actor = name, error = %e, "Actor start failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
            return;
        }

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        if let Err(e) = actor.run().await {
            tracing::error!(actor = name, error = %e, "Actor run failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
        }

        if let Err(e) = actor.stop().await {
            tracing::warn!(actor = name, error = %e, "Actor stop failed");
        }

        tracing::info!(actor = name, "Actor task stopped");
    })
}

/// Spawn STT actor in a tokio task.
fn spawn_stt_actor(
    mut actor: speech::SttActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = actor.name();
        tracing::info!(actor = name, "Actor task started");

        if let Err(e) = actor.start(bus.clone()).await {
            tracing::error!(actor = name, error = %e, "Actor start failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
            return;
        }

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        if let Err(e) = actor.run().await {
            tracing::error!(actor = name, error = %e, "Actor run failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
        }

        if let Err(e) = actor.stop().await {
            tracing::warn!(actor = name, error = %e, "Actor stop failed");
        }

        tracing::info!(actor = name, "Actor task stopped");
    })
}

/// Spawn TTS actor in a tokio task.
fn spawn_tts_actor(
    mut actor: speech::TtsActor,
    bus: std::sync::Arc<EventBus>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let name = actor.name();
        tracing::info!(actor = name, "Actor task started");

        if let Err(e) = actor.start(bus.clone()).await {
            tracing::error!(actor = name, error = %e, "Actor start failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
            return;
        }

        // Emit ActorReady
        let _ = bus
            .broadcast(Event::System(SystemEvent::ActorReady { actor_name: name }))
            .await;

        if let Err(e) = actor.run().await {
            tracing::error!(actor = name, error = %e, "Actor run failed");
            let _ = bus
                .broadcast(Event::System(SystemEvent::ActorFailed {
                    actor: name.to_string(),
                    reason: e.to_string(),
                }))
                .await;
        }

        if let Err(e) = actor.stop().await {
            tracing::warn!(actor = name, error = %e, "Actor stop failed");
        }

        tracing::info!(actor = name, "Actor task stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boot_sequence_completes() {
        let result = boot().await;
        assert!(result.is_ok());

        let boot_result = result.unwrap();
        assert!(boot_result.actor_handles.len() > 0);
        assert!(boot_result.expected_actors.len() > 0);
    }

    #[tokio::test]
    async fn load_config_stub_succeeds() {
        let result = load_config().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn init_encryption_stub_succeeds() {
        let result = init_encryption().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn spawn_actors_creates_expected_list() {
        let bus = Arc::new(EventBus::new());
        let result = spawn_actors(bus).await;
        assert!(result.is_ok());

        let (handles, expected) = result.unwrap();
        assert_eq!(handles.len(), expected.len());
        assert!(expected.contains(&"soul"));
        assert!(expected.contains(&"inference"));
        assert!(expected.contains(&"memory"));
        assert!(expected.contains(&"platform"));
        assert!(expected.contains(&"ctp"));
        assert!(expected.contains(&"prompt"));
        assert!(expected.contains(&"stt"));
        assert!(expected.contains(&"tts"));
    }
}
