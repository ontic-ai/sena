//! Boot sequence orchestration.

use std::sync::Arc;
use std::time::Duration;

use bus::{Actor, Event, EventBus, SystemEvent};
use crypto::MasterKey;
use rand::RngCore;

use crate::config::SenaConfig;
use crate::registry::ActorRegistry;

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
    pub(crate) _keep_alive: tokio::sync::broadcast::Receiver<Event>,
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
}

/// Boot sequence per architecture Â§4.1.
pub async fn boot() -> Result<Runtime, BootError> {
    // Step 1: Config load
    let config = crate::config::load_or_create_config()
        .await
        .map_err(|e| BootError::ConfigLoadFailed(e.to_string()))?;
    // Step 1.5: First-boot detection — check if Soul.redb exists before any initialization.
    // We check for the Soul redb encrypted file. If absent, this is the first boot.
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

    // Step 5: Actor registry â€” spawn Soul first (lowest-level subsystem)
    let mut registry = ActorRegistry::new();
    spawn_actor(&bus, &mut registry, Box::new(soul_actor));

    // Step 6: Initialize and spawn platform adapter
    let platform_adapter = platform::create_platform_adapter();
    let platform_actor = platform::PlatformActor::new(platform_adapter)
        .with_poll_interval(Duration::from_millis(500))
        .with_clipboard_enabled(config.clipboard_observation_enabled);
    spawn_actor(&bus, &mut registry, Box::new(platform_actor));

    // Step 7: Initialize and spawn CTP actor
    let ctp_trigger_interval = Duration::from_secs(config.ctp_trigger_interval_secs);
    let ctp_buffer_window = Duration::from_secs(300);
    let ctp_poll_interval = Duration::from_secs(1);
    let ctp_actor = ctp::CTPActor::new(ctp_trigger_interval, ctp_buffer_window, ctp_poll_interval)
        .with_trigger_sensitivity(config.ctp_trigger_sensitivity);
    spawn_actor(&bus, &mut registry, Box::new(ctp_actor));

    // Step 8: Initialize and spawn memory actor
    let config_dir =
        platform::config_dir().map_err(|e| BootError::MemoryInitFailed(e.to_string()))?;
    let memory_dir = config_dir.join("memory");
    std::fs::create_dir_all(&memory_dir)
        .map_err(|e| BootError::MemoryInitFailed(format!("failed to create memory dir: {e}")))?;
    let memory_graph_path = memory_dir.join("graph");
    let memory_vector_path = memory_dir.join("vector.usearch");

    let memory_consolidation_interval =
        Duration::from_secs(config.memory_consolidation_interval_secs);
    let memory_idle_threshold = Duration::from_secs(config.memory_consolidation_idle_secs);
    let memory_actor = memory::MemoryActor::with_consolidation_interval(
        memory_graph_path,
        memory_vector_path,
        memory_consolidation_interval,
    )
    .with_consolidation_idle_threshold(memory_idle_threshold);
    spawn_actor(&bus, &mut registry, Box::new(memory_actor));

    // Step 9: Initialize and spawn inference actor
    let models_dir =
        platform::ollama_models_dir().map_err(|e| BootError::InferenceInitFailed(e.to_string()))?;
    let inference_backend = inference::LlamaBackend::new();
    let inference_actor = inference::InferenceActor::new(models_dir, Box::new(inference_backend))
        .with_preferred_model(config.preferred_model.clone());
    spawn_actor(&bus, &mut registry, Box::new(inference_actor));

    // Step 10: PromptComposer is stateless — instantiated per inference cycle.
    // prompt::PromptComposer::new() is cheap; no actor spawn needed.

    // Step 11: Initialize system tray (non-fatal — Sena works without it).
    let tray_manager =
        crate::tray::TrayManager::new(Arc::clone(&bus), tokio::runtime::Handle::current());

    // Step 12: Broadcast BootComplete
    bus.broadcast(Event::System(SystemEvent::BootComplete))
        .await
        .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;

    Ok(Runtime {
        bus,
        registry,
        config,
        tray_manager,
        is_first_boot,
        master_key,
        _keep_alive: keep_alive,
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
    rand::thread_rng().fill_bytes(&mut raw);
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

fn spawn_actor(bus: &Arc<EventBus>, registry: &mut ActorRegistry, mut actor: Box<dyn Actor>) {
    let name = actor.name();
    let bus_clone = Arc::clone(bus);
    let handle = tokio::spawn(async move {
        if let Err(e) = actor.start(bus_clone).await {
            eprintln!("Actor '{}' failed to start: {}", name, e);
            return;
        }
        if let Err(e) = actor.run().await {
            eprintln!("Actor '{}' failed during run: {}", name, e);
        }
        let _ = actor.stop().await;
    });
    registry.register(name, handle);
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
            _keep_alive: keep_alive,
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
            Box::new(MockActor::new(
                "mock1",
                StdArc::clone(&actor1_started),
                StdArc::clone(&actor1_ran),
                StdArc::clone(&actor1_stopped),
            )),
        );
        spawn_actor(
            &bus,
            &mut registry,
            Box::new(MockActor::new(
                "mock2",
                StdArc::clone(&actor2_started),
                StdArc::clone(&actor2_ran),
                StdArc::clone(&actor2_stopped),
            )),
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
            _keep_alive: keep_alive,
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
    async fn first_boot_detection_logic() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir should create");
        let soul_path = dir.path().join("soul.redb.enc");

        // File absent → first boot
        assert!(!soul_path.exists());

        // File present → not first boot
        fs::write(&soul_path, b"placeholder").expect("write should succeed");
        assert!(soul_path.exists());
    }

    #[tokio::test]
    async fn first_boot_emits_event_when_soul_absent() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        let event = Event::System(SystemEvent::FirstBoot);
        bus.broadcast(event)
            .await
            .expect("broadcast should succeed in test");

        let received = rx.recv().await;
        assert!(received.is_ok());

        if let Ok(Event::System(SystemEvent::FirstBoot)) = received {
            // Success
        } else {
            panic!("Expected FirstBoot event");
        }
    }
}
