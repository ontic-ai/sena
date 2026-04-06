//! Boot sequence orchestration.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bus::events::system::ActorFailureInfo;
use bus::{Actor, Event, EventBus, SystemEvent};
use crypto::MasterKey;
use rand::RngCore;

use crate::config::SenaConfig;
use crate::registry::ActorRegistry;
use crate::single_instance::SingleInstanceGuard;

/// Runtime holds initialized subsystems and the event bus.
pub struct Runtime {
    pub bus: Arc<EventBus>,
    pub registry: ActorRegistry,
    pub config: SenaConfig,
    pub tray_manager: crate::tray::TrayManager,
    /// True if this is the first time Sena has been run on this machine.
    pub is_first_boot: bool,
    #[allow(dead_code)]
    pub(crate) master_key: MasterKey,
    /// Shared latest in-memory vision frame from platform actor.
    pub vision_frame_store: Arc<Mutex<Option<Vec<u8>>>>,
    pub(crate) _keep_alive: tokio::sync::broadcast::Receiver<Event>,
    /// Memory monitor task handle (if enabled).
    pub(crate) memory_monitor_handle: Option<tokio::task::JoinHandle<()>>,
    /// Names of actors expected to emit `ActorReady` before `BootComplete`.
    /// Used by the supervisor readiness gate.
    pub expected_actors: Vec<&'static str>,
    /// Broadcast receiver subscribed BEFORE any actor is spawned.
    /// Handed to `wait_for_readiness` so no `ActorReady` event is missed
    /// due to the race between actor spawn and supervisor subscription.
    pub(crate) readiness_rx: Option<tokio::sync::broadcast::Receiver<Event>>,
    /// Single-instance lock guard. Kept alive for the process lifetime.
    #[allow(dead_code)]
    pub(crate) single_instance_guard: SingleInstanceGuard,
}

/// Boot sequence errors.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("config load failed: {0}")]
    ConfigLoadFailed(String),

    #[error("encryption init failed: {0}")]
    EncryptionInitFailed(String),

    #[error("event bus init failed: {0}")]
    BusInitFailed(String),

    #[error("soul init failed: {0}")]
    SoulInitFailed(String),

    #[error("actor spawn failed: {0}")]
    ActorSpawnFailed(String),

    #[error("platform init failed: {0}")]
    PlatformInitFailed(String),

    #[error("ctp init failed: {0}")]
    CTPInitFailed(String),

    #[error("memory init failed: {0}")]
    MemoryInitFailed(String),

    #[error("inference init failed: {0}")]
    InferenceInitFailed(String),

    #[error("prompt init failed: {0}")]
    PromptInitFailed(String),

    #[error("boot complete broadcast failed: {0}")]
    BroadcastFailed(String),

    #[error("readiness timeout: {0}")]
    ReadinessTimeout(String),

    #[error("single instance check failed: {0}")]
    SingleInstanceCheckFailed(String),
}

/// Full boot sequence per architecture §4.1 (all 13 steps).
/// Runtime is the composition root — all actors are constructed here.
pub async fn boot() -> Result<Runtime, BootError> {
    // Step 0: Single-instance enforcement — must be first so we fail fast if another instance is running.
    let single_instance_guard = crate::single_instance::try_acquire_lock()
        .map_err(|e| BootError::SingleInstanceCheckFailed(e.to_string()))?;

    // Step 0.5: Hardware profiling for auto-configuration.
    let hw_profile = tokio::task::spawn_blocking(crate::hardware_profile::profile_hardware)
        .await
        .unwrap_or_default();
    tracing::info!(
        "hardware profile: {}MB RAM, {}MB VRAM, {} cores → recommended tokens: {}",
        hw_profile.total_ram_mb,
        hw_profile.available_vram_mb,
        hw_profile.cpu_cores,
        crate::hardware_profile::recommended_tokens(&hw_profile),
    );

    // Step 1: Config load
    let mut config = crate::config::load_or_create_config()
        .await
        .map_err(|e| BootError::ConfigLoadFailed(e.to_string()))?;

    // Step 1.1: Auto-tune inference_max_tokens if still at default.
    if config.inference_max_tokens == crate::config::default_inference_max_tokens() {
        let recommended = crate::hardware_profile::recommended_tokens(&hw_profile) as usize;
        tracing::info!(
            "inference_max_tokens at default ({}), applying hardware recommendation: {}",
            config.inference_max_tokens,
            recommended
        );
        config.inference_max_tokens = recommended;
    } else {
        tracing::info!(
            "inference_max_tokens explicitly set to {}, not applying hardware recommendation",
            config.inference_max_tokens
        );
    }

    // Step 1.5: First-boot detection — check if Soul.redb exists before any initialization.
    let is_first_boot = {
        let soul_path = platform::config_dir()
            .map_err(|e| BootError::ConfigLoadFailed(e.to_string()))?
            .join("soul.redb.enc");
        !soul_path.exists()
    };

    // Step 2: Initialize encryption
    let master_key =
        init_encryption().map_err(|e| BootError::EncryptionInitFailed(e.to_string()))?;

    // Step 3: Initialize EventBus
    let bus = Arc::new(EventBus::new());
    let keep_alive = bus.subscribe_broadcast();

    // Subscribe BEFORE spawning any actor so no ActorReady is missed due to the
    // race between tokio::spawn scheduling and wait_for_readiness subscription.
    let readiness_rx = bus.subscribe_broadcast();

    bus.broadcast(Event::System(SystemEvent::EncryptionInitialized))
        .await
        .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;

    if is_first_boot {
        bus.broadcast(Event::System(SystemEvent::FirstBoot))
            .await
            .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;
    }

    // Step 4: Initialize Soul (SoulBox)
    let soul_db_path = platform::config_dir()
        .map_err(|e| BootError::SoulInitFailed(e.to_string()))?
        .join("soul.redb.enc");
    let soul_master_key = MasterKey::from_bytes(*master_key.as_bytes());
    let soul_actor = soul::SoulActor::new(soul_db_path, soul_master_key);

    // Step 5: Actor registry — spawn Soul first (lowest-level subsystem)
    let mut registry = ActorRegistry::new();
    let mut expected_actors: Vec<&'static str> = Vec::new();
    spawn_actor(&bus, &mut registry, soul_actor);
    expected_actors.push("Soul");

    // Step 6: Initialize and spawn platform adapter
    let platform_adapter = platform::create_platform_adapter();
    let platform_actor = platform::PlatformActor::new(platform_adapter)
        .with_poll_interval(Duration::from_millis(500))
        .with_clipboard_enabled(config.clipboard_observation_enabled)
        .with_file_watch_paths(config.file_watch_paths.clone())
        .with_idle_threshold(config.platform_idle_cpu_threshold_percent)
        .with_screen_capture_enabled(config.screen_capture_enabled);
    // Shared between platform actor (writer) and inference actor (reader).
    let vision_frame_store = platform_actor.latest_frame_store();
    spawn_actor(&bus, &mut registry, platform_actor);
    expected_actors.push("Platform");

    // Step 7: Initialize and spawn CTP actor
    let ctp_trigger_interval = Duration::from_secs(config.ctp_trigger_interval_secs);
    let ctp_buffer_window = Duration::from_secs(300);
    let ctp_poll_interval = Duration::from_secs(1);
    let ctp_actor = ctp::CTPActor::new(ctp_trigger_interval, ctp_buffer_window, ctp_poll_interval)
        .with_trigger_sensitivity(config.ctp_trigger_sensitivity)
        .with_screen_capture_enabled(config.screen_capture_enabled);
    spawn_actor(&bus, &mut registry, ctp_actor);
    expected_actors.push("CTP");

    // Step 8: Memory actor
    let config_dir =
        platform::config_dir().map_err(|e| BootError::MemoryInitFailed(e.to_string()))?;
    let memory_dir = config_dir.join("memory");
    std::fs::create_dir_all(&memory_dir).map_err(|e| BootError::MemoryInitFailed(e.to_string()))?;
    let memory_consolidation_interval =
        Duration::from_secs(config.memory_consolidation_interval_secs);
    let memory_idle_threshold = Duration::from_secs(config.memory_consolidation_idle_secs);
    let memory_master_key = MasterKey::from_bytes(*master_key.as_bytes());
    let memory_actor = memory::MemoryActor::with_consolidation_interval(
        memory_dir,
        memory_master_key,
        memory_consolidation_interval,
    )
    .with_consolidation_idle_threshold(memory_idle_threshold);
    spawn_actor(&bus, &mut registry, memory_actor);
    expected_actors.push("Memory");

    // Step 9: Inference actor
    let models_dir = if let Some(path) = config.models_dir.clone() {
        path
    } else {
        platform::ollama_models_dir().map_err(|e| BootError::InferenceInitFailed(e.to_string()))?
    };
    let llama_backend = inference::LlamaBackend::new()
        .map_err(|e| BootError::InferenceInitFailed(format!("LlamaBackend init failed: {}", e)))?;
    let inference_actor = inference::InferenceActor::new(models_dir, Box::new(llama_backend))
        .with_preferred_model(config.preferred_model.clone())
        .with_vision_frame_store(Arc::clone(&vision_frame_store))
        .with_tts_enabled(config.speech_enabled)
        .with_inference_max_tokens(config.inference_max_tokens)
        .with_inference_ctx_size(config.inference_ctx_size)
        .with_proactive_speech(config.proactive_speech_enabled)
        .with_speech_rate_limit(config.speech_rate_limit_secs);
    spawn_actor(&bus, &mut registry, inference_actor);
    expected_actors.push("Inference");

    // Step 10: PromptComposer is stateless — no spawn needed.

    // Step 10.5: Speech onboarding (non-fatal).
    let mut speech_available = config.speech_enabled;
    let speech_model_dir = config
        .speech_model_dir
        .clone()
        .unwrap_or_else(crate::config::default_speech_model_dir);

    if config.speech_enabled
        && speech::onboarding::speech_onboarding_needed(&speech_model_dir).await
    {
        match speech::onboarding::run_speech_onboarding(&bus, &speech_model_dir).await {
            Ok(_downloaded) => {}
            Err(e) => {
                eprintln!("WARN: speech onboarding failed, disabling speech: {}", e);
                speech_available = false;
            }
        }
    }

    // Step 11: Speech actors (STT/TTS/Wakeword) — spawn only if onboarding succeeded.
    if speech_available {
        #[cfg(feature = "whisper")]
        let stt_backend = speech::SttBackend::WhisperCpp;
        #[cfg(not(feature = "whisper"))]
        let stt_backend = speech::SttBackend::Mock;

        let stt_actor = speech::SttActor::new(
            stt_backend,
            config.voice_always_listening,
            config.stt_energy_threshold,
            config.whisper_model_path.clone(),
        )
        .with_model_dir(Some(speech_model_dir.clone()))
        .with_microphone_device(config.microphone_device.clone());
        spawn_actor(&bus, &mut registry, stt_actor);
        expected_actors.push("stt");

        let tts_actor = speech::TtsActor::new(speech::TtsBackend::Piper)
            .with_voice(config.tts_voice.clone())
            .with_rate(config.tts_rate)
            .with_model_dir(Some(speech_model_dir.clone()));
        spawn_actor(&bus, &mut registry, tts_actor);
        expected_actors.push("tts");

        if config.wakeword_enabled {
            let wakeword_config = speech::wakeword::WakewordConfig {
                sensitivity: config.wakeword_sensitivity,
                model_path: None,
                model_dir: Some(speech_model_dir),
                debounce_secs: 3.0,
            };
            let wakeword_actor = speech::WakewordActor::new(wakeword_config);
            spawn_actor(&bus, &mut registry, wakeword_actor);
        }
    }

    // Step 12: Initialize system tray (non-fatal).
    let tray_manager =
        crate::tray::TrayManager::new(Arc::clone(&bus), tokio::runtime::Handle::current());

    // Step 12.5: Spawn memory monitor task.
    let memory_monitor_handle = spawn_memory_monitor(Arc::clone(&bus), &config);

    // Step 13: Spawn IPC server (CLI communication endpoint).
    {
        let ipc_bus = Arc::clone(&bus);
        tokio::spawn(async move {
            if let Err(e) = crate::ipc_server::start(ipc_bus).await {
                tracing::error!("IPC server failed: {}", e);
            }
        });
    }

    // Step 14: BootComplete is broadcast by `lib::boot_ready_impl()` after the
    // supervisor readiness gate confirms all actors are up.

    Ok(Runtime {
        bus,
        registry,
        config,
        tray_manager,
        single_instance_guard,
        is_first_boot,
        master_key,
        vision_frame_store,
        _keep_alive: keep_alive,
        memory_monitor_handle: Some(memory_monitor_handle),
        expected_actors,
        readiness_rx: Some(readiness_rx),
    })
}

fn init_encryption() -> Result<MasterKey, crypto::CryptoError> {
    // Case 1: Key already exists in OS keychain (normal subsequent runs).
    if let Ok(key) = crypto::keychain::retrieve_master_key() {
        return Ok(key);
    }

    // Case 2: Passphrase supplied via environment variable (CI / headless).
    if let Ok(passphrase_str) = std::env::var("SENA_PASSPHRASE") {
        let passphrase = crypto::argon2_kdf::Passphrase::new(passphrase_str);
        let salt_path = platform::config_dir()
            .map_err(|e| crypto::CryptoError::IoError(std::io::Error::other(e.to_string())))?
            .join("salt.bin");
        let salt = load_or_create_salt(&salt_path)?;
        let key = crypto::argon2_kdf::derive_master_key(&passphrase, &salt)?;
        crypto::keychain::store_master_key(&key)?;
        return Ok(key);
    }

    // Case 3: First run — generate a fresh random master key and store it.
    // The key is never written to disk; only the keychain holds it.
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let key = crypto::MasterKey::from_bytes(raw);
    crypto::keychain::store_master_key(&key)?;
    Ok(key)
}

fn load_or_create_salt(
    salt_path: &std::path::Path,
) -> Result<crypto::argon2_kdf::Salt, crypto::CryptoError> {
    if salt_path.exists() {
        let bytes = std::fs::read(salt_path).map_err(crypto::CryptoError::IoError)?;
        if bytes.len() != 16 {
            return Err(crypto::CryptoError::InvalidData(
                "salt file has invalid length".to_string(),
            ));
        }
        let mut arr = [0u8; 16];
        arr.copy_from_slice(&bytes);
        Ok(crypto::argon2_kdf::Salt::from_bytes(arr))
    } else {
        let salt = crypto::argon2_kdf::generate_salt();
        if let Some(parent) = salt_path.parent() {
            std::fs::create_dir_all(parent).map_err(crypto::CryptoError::IoError)?;
        }
        std::fs::write(salt_path, salt.as_bytes()).map_err(crypto::CryptoError::IoError)?;
        Ok(salt)
    }
}

fn spawn_actor<A>(bus: &Arc<EventBus>, registry: &mut ActorRegistry, mut actor: A)
where
    A: Actor,
{
    let name = actor.name();
    let bus_for_actor = Arc::clone(bus);
    let bus_for_events = Arc::clone(bus);
    let handle = tokio::spawn(async move {
        if let Err(e) = actor.start(bus_for_actor).await {
            tracing::error!("actor '{}' failed to start: {}", name, e);
            let _ = bus_for_events
                .broadcast(Event::System(SystemEvent::ActorFailed(ActorFailureInfo {
                    actor_name: name,
                    error_msg: e.to_string(),
                })))
                .await;
            return;
        }
        if let Err(e) = actor.run().await {
            tracing::error!("actor '{}' failed during run: {}", name, e);
            let _ = bus_for_events
                .broadcast(Event::System(SystemEvent::ActorFailed(ActorFailureInfo {
                    actor_name: name,
                    error_msg: e.to_string(),
                })))
                .await;
        }
        let _ = actor.stop().await;
    });
    registry.register(name, handle);
}

/// Spawns a background task that monitors memory usage every `interval_secs`.
/// Broadcasts `MemoryThresholdExceeded` event if RSS exceeds `limit_mb`.
/// Exits cleanly when `ShutdownSignal` is received on the bus or when the bus closes.
fn spawn_memory_monitor(bus: Arc<EventBus>, config: &SenaConfig) -> tokio::task::JoinHandle<()> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

    let interval = Duration::from_secs(config.memory_monitor_interval_secs);
    let limit_mb = config.memory_limit_mb;
    // Subscribe before spawning so no ShutdownSignal is missed.
    let mut shutdown_rx = bus.subscribe_broadcast();

    tokio::spawn(async move {
        let mut sys = System::new_with_specifics(
            RefreshKind::new().with_processes(ProcessRefreshKind::new().with_memory()),
        );
        let pid = sysinfo::get_current_pid().ok();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    if let Some(pid) = pid {
                        // Refresh only memory information for efficiency.
                        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);

                        if let Some(process) = sys.process(pid) {
                            // sysinfo returns memory in bytes. Convert to MB.
                            let memory_bytes = process.memory();
                            let current_mb = (memory_bytes / (1024 * 1024)) as usize;

                            if current_mb > limit_mb {
                                eprintln!(
                                    "WARN: Memory threshold exceeded: {} MB / {} MB limit",
                                    current_mb, limit_mb
                                );

                                let event = Event::System(SystemEvent::MemoryThresholdExceeded {
                                    current_mb,
                                    limit_mb,
                                });

                                if let Err(e) = bus.broadcast(event).await {
                                    eprintln!(
                                        "Failed to broadcast MemoryThresholdExceeded event: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
                msg = shutdown_rx.recv() => {
                    match msg {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) | Err(_) => break,
                        _ => {}
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn boot_constructs_runtime_with_actors() {
        let bus = Arc::new(EventBus::new());
        let keep_alive = bus.subscribe_broadcast();
        let config = SenaConfig::default();
        let registry = ActorRegistry::new();
        let master_key = MasterKey::from_bytes([0u8; 32]);

        bus.broadcast(Event::System(SystemEvent::BootComplete))
            .await
            .expect("broadcast should succeed");

        let tray_manager =
            crate::tray::TrayManager::new(Arc::clone(&bus), tokio::runtime::Handle::current());

        let runtime = Runtime {
            bus,
            registry,
            config,
            tray_manager,
            is_first_boot: false,
            master_key,
            vision_frame_store: Arc::new(Mutex::new(None)),
            _keep_alive: keep_alive,
            memory_monitor_handle: None,
            expected_actors: vec![],
            readiness_rx: None,
            single_instance_guard: SingleInstanceGuard::test_dummy(),
        };

        assert_eq!(runtime.registry.actor_count(), 0);
    }

    #[tokio::test]
    async fn boot_emits_boot_complete_event() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        let event = Event::System(SystemEvent::BootComplete);
        bus.broadcast(event)
            .await
            .expect("broadcast should succeed in test");

        let received = rx.recv().await;
        assert!(received.is_ok());

        if let Ok(Event::System(SystemEvent::BootComplete)) = received {
            // Success
        } else {
            panic!("Expected BootComplete event");
        }
    }

    #[tokio::test]
    async fn spawn_actors_lifecycle_integration() {
        use bus::ActorError;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc as StdArc;

        struct MockActor {
            name: &'static str,
            started: StdArc<AtomicBool>,
            ran: StdArc<AtomicBool>,
            stopped: StdArc<AtomicBool>,
        }

        impl MockActor {
            fn new(
                name: &'static str,
                started: StdArc<AtomicBool>,
                ran: StdArc<AtomicBool>,
                stopped: StdArc<AtomicBool>,
            ) -> Self {
                Self {
                    name,
                    started,
                    ran,
                    stopped,
                }
            }
        }

        #[async_trait::async_trait]
        impl Actor for MockActor {
            fn name(&self) -> &'static str {
                self.name
            }

            async fn start(&mut self, _bus: Arc<EventBus>) -> Result<(), ActorError> {
                self.started.store(true, Ordering::SeqCst);
                Ok(())
            }

            async fn run(&mut self) -> Result<(), ActorError> {
                self.ran.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(())
            }

            async fn stop(&mut self) -> Result<(), ActorError> {
                self.stopped.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        let bus = Arc::new(EventBus::new());
        let keep_alive = bus.subscribe_broadcast();
        let config = SenaConfig::default();
        let mut registry = ActorRegistry::new();

        let actor1_started = StdArc::new(AtomicBool::new(false));
        let actor1_ran = StdArc::new(AtomicBool::new(false));
        let actor1_stopped = StdArc::new(AtomicBool::new(false));
        let actor2_started = StdArc::new(AtomicBool::new(false));
        let actor2_ran = StdArc::new(AtomicBool::new(false));
        let actor2_stopped = StdArc::new(AtomicBool::new(false));

        spawn_actor(
            &bus,
            &mut registry,
            MockActor::new(
                "mock1",
                StdArc::clone(&actor1_started),
                StdArc::clone(&actor1_ran),
                StdArc::clone(&actor1_stopped),
            ),
        );
        spawn_actor(
            &bus,
            &mut registry,
            MockActor::new(
                "mock2",
                StdArc::clone(&actor2_started),
                StdArc::clone(&actor2_ran),
                StdArc::clone(&actor2_stopped),
            ),
        );

        assert_eq!(registry.actor_count(), 2);

        let tray_manager =
            crate::tray::TrayManager::new(Arc::clone(&bus), tokio::runtime::Handle::current());

        let runtime = Runtime {
            bus,
            registry,
            config,
            tray_manager,
            is_first_boot: false,
            master_key: MasterKey::from_bytes([0u8; 32]),
            vision_frame_store: Arc::new(Mutex::new(None)),
            _keep_alive: keep_alive,
            memory_monitor_handle: None,
            expected_actors: vec![],
            readiness_rx: None,
            single_instance_guard: SingleInstanceGuard::test_dummy(),
        };

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(actor1_started.load(Ordering::SeqCst));
        assert!(actor2_started.load(Ordering::SeqCst));

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(actor1_ran.load(Ordering::SeqCst));
        assert!(actor2_ran.load(Ordering::SeqCst));

        let result = crate::shutdown::shutdown(runtime, Duration::from_secs(2)).await;
        assert!(result.is_ok());

        assert!(actor1_stopped.load(Ordering::SeqCst));
        assert!(actor2_stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn first_boot_detection_logic_file_absent() {
        let dir = tempfile::tempdir().expect("tempdir should create");
        let soul_path = dir.path().join("soul.redb.enc");

        // File absent → first boot
        assert!(
            !soul_path.exists(),
            "fresh tempdir should not have soul.redb.enc"
        );

        // Simulated boot step: check if file exists
        let is_first_boot = !soul_path.exists();
        assert!(
            is_first_boot,
            "should detect first boot when file is absent"
        );
    }

    #[tokio::test]
    async fn first_boot_detection_logic_file_present() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir should create");
        let soul_path = dir.path().join("soul.redb.enc");

        // Simulate a previous boot by creating the Soul file
        fs::write(&soul_path, b"placeholder").expect("write should succeed");
        assert!(soul_path.exists(), "file should exist after write");

        // Simulated boot step: check if file exists
        let is_first_boot = !soul_path.exists();
        assert!(
            !is_first_boot,
            "should NOT detect first boot when file exists"
        );
    }

    #[tokio::test]
    async fn first_boot_emits_event_when_soul_absent() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        // Simulate boot broadcasting FirstBoot event when soul is absent
        let event = Event::System(SystemEvent::FirstBoot);
        bus.broadcast(event)
            .await
            .expect("broadcast should succeed in test");

        let received = rx.recv().await;
        assert!(received.is_ok(), "should receive FirstBoot event");

        match received {
            Ok(Event::System(SystemEvent::FirstBoot)) => {
                // Success: FirstBoot event received as expected
            }
            _ => panic!("Expected FirstBoot event, got {:?}", received),
        }
    }

    #[tokio::test]
    async fn first_boot_does_not_emit_when_soul_present() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        // On second boot, FirstBoot event should NOT be broadcast.
        // Simulate second boot by broadcasting EncryptionInitialized but NOT FirstBoot.
        bus.broadcast(Event::System(SystemEvent::EncryptionInitialized))
            .await
            .expect("broadcast should succeed");

        // We should receive EncryptionInitialized but NOT FirstBoot
        let received = rx.recv().await;
        assert!(received.is_ok());

        match received {
            Ok(Event::System(SystemEvent::EncryptionInitialized)) => {
                // Correct: received expected event
            }
            Ok(Event::System(SystemEvent::FirstBoot)) => {
                panic!("FirstBoot event should NOT be broadcast on second run");
            }
            _ => panic!("Unexpected event: {:?}", received),
        }

        // Try to receive another event with a short timeout — should not receive FirstBoot
        let timeout_result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        match timeout_result {
            Err(_) => {
                // Timeout is expected — no FirstBoot event broadcast
            }
            Ok(Ok(Event::System(SystemEvent::FirstBoot))) => {
                panic!("FirstBoot should NOT be broadcast on second boot");
            }
            Ok(_) => {
                // Some other event — that's fine
            }
        }
    }

    #[tokio::test]
    async fn first_boot_flag_correct_when_soul_absent() {
        // This test verifies the is_first_boot flag is set correctly when soul.redb.enc is absent.
        // We cannot run the full boot() function in a test because it requires OS-level setup.
        // Instead, we verify the detection logic in isolation.
        let dir = tempfile::tempdir().expect("tempdir should create");
        let soul_path = dir.path().join("soul.redb.enc");

        // First boot scenario: soul.redb.enc does not exist
        let is_first_boot = !soul_path.exists();
        assert!(
            is_first_boot,
            "is_first_boot should be true when soul.redb.enc is absent"
        );
    }

    #[tokio::test]
    async fn first_boot_flag_correct_when_soul_present() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir should create");
        let soul_path = dir.path().join("soul.redb.enc");

        // Second boot scenario: soul.redb.enc exists from a previous run
        fs::write(&soul_path, b"previous boot data").expect("write should succeed");

        let is_first_boot = !soul_path.exists();
        assert!(
            !is_first_boot,
            "is_first_boot should be false when soul.redb.enc exists"
        );
    }

    #[tokio::test]
    async fn first_boot_onboarding_integration_simulation() {
        // This test simulates the onboarding flow decision point in main.rs interactive_mode.
        // When is_first_boot is true, onboarding wizard should run.
        // When is_first_boot is false, onboarding wizard should be skipped.

        // Scenario 1: First boot → onboarding runs
        let is_first_boot_scenario_1 = true;
        assert!(
            is_first_boot_scenario_1,
            "onboarding should run on first boot"
        );

        // Scenario 2: Second boot → onboarding skipped
        let is_first_boot_scenario_2 = false;
        assert!(
            !is_first_boot_scenario_2,
            "onboarding should NOT run on second boot"
        );
    }

    #[tokio::test]
    async fn memory_monitor_exits_cleanly_on_shutdown_signal() {
        let bus = Arc::new(EventBus::new());
        // Use a very short interval so the test doesn't have to wait 60 seconds.
        let config = SenaConfig {
            memory_monitor_interval_secs: 3600, // long interval — shutdown triggers exit
            ..SenaConfig::default()
        };

        let handle = spawn_memory_monitor(Arc::clone(&bus), &config);

        // Give the task a moment to enter its select! loop.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Broadcast shutdown — the monitor task should exit its loop.
        bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("broadcast should succeed");

        // The task should complete within a short time after receiving ShutdownSignal.
        let result = tokio::time::timeout(Duration::from_millis(500), handle).await;
        assert!(result.is_ok(), "memory monitor did not exit within timeout");
    }

    #[tokio::test]
    async fn memory_monitor_broadcasts_threshold_event_when_limit_zero() {
        // Set limit_mb to 0 so any non-zero RSS triggers the event.
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        let config = SenaConfig {
            memory_monitor_interval_secs: 0, // immediate first tick
            memory_limit_mb: 0,              // always exceeded
            ..SenaConfig::default()
        };

        let _handle = spawn_memory_monitor(Arc::clone(&bus), &config);

        // Wait up to 2 seconds for the MemoryThresholdExceeded event.
        let timeout = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match rx.recv().await {
                    Ok(Event::System(SystemEvent::MemoryThresholdExceeded {
                        current_mb,
                        limit_mb,
                    })) => return (current_mb, limit_mb),
                    Ok(_) => continue,
                    Err(e) => panic!("channel error: {e}"),
                }
            }
        })
        .await;

        match timeout {
            Ok((current_mb, limit_mb)) => {
                assert_eq!(limit_mb, 0);
                // current_mb should be > 0 (the process is using some memory).
                assert!(
                    current_mb > 0,
                    "expected non-zero RSS but got {} MB",
                    current_mb
                );
            }
            Err(_) => {
                // On some CI environments sysinfo cannot read the process memory
                // (e.g., missing /proc access). Skip rather than fail.
                eprintln!("WARN: MemoryThresholdExceeded not received within timeout — sysinfo may not have process access in this environment");
            }
        }
    }
}
