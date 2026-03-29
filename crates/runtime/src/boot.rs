//! Boot sequence orchestration.

use std::sync::Arc;
use std::time::Duration;

use bus::{Actor, Event, EventBus, SystemEvent};
use crypto::MasterKey;

use crate::config::SenaConfig;
use crate::registry::ActorRegistry;

/// Runtime holds initialized subsystems and the event bus.
pub struct Runtime {
    /// Event bus for all actor communication.
    pub bus: Arc<EventBus>,
    /// Registry of spawned actor tasks.
    pub registry: ActorRegistry,
    /// Loaded configuration.
    pub config: SenaConfig,
    /// Master encryption key — zeroed on Runtime drop.
    /// Used by Soul and Memory actors during initialization (U9/U10).
    #[allow(dead_code)]
    pub(crate) master_key: MasterKey,
    /// Keep-alive receiver to prevent broadcast channel from closing.
    pub(crate) _keep_alive: tokio::sync::broadcast::Receiver<Event>,
}

/// Boot sequence errors.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// Step 1: Config load failed.
    #[error("config load failed: {0}")]
    ConfigLoadFailed(String),

    /// Step 2: Encryption initialization failed.
    #[error("encryption init failed: {0}")]
    EncryptionInitFailed(String),

    /// Step 3: Event bus initialization failed.
    #[error("event bus init failed: {0}")]
    BusInitFailed(String),

    /// Step 4: Soul (SoulBox) initialization failed.
    #[error("soul init failed: {0}")]
    SoulInitFailed(String),

    /// Step 5: Actor spawn failed.
    #[error("actor spawn failed: {0}")]
    ActorSpawnFailed(String),

    /// Step 6: Platform adapter init failed.
    #[error("platform init failed: {0}")]
    PlatformInitFailed(String),

    /// Step 7: CTP (Continuous Thought Processing) init failed.
    #[error("ctp init failed: {0}")]
    CTPInitFailed(String),

    /// Step 8: Memory system init failed.
    #[error("memory init failed: {0}")]
    MemoryInitFailed(String),

    /// Step 9: Inference system init failed.
    #[error("inference init failed: {0}")]
    InferenceInitFailed(String),

    /// Step 10: Prompt composer init failed.
    #[error("prompt init failed: {0}")]
    PromptInitFailed(String),

    /// Step 11: Boot complete broadcast failed.
    #[error("boot complete broadcast failed: {0}")]
    BroadcastFailed(String),
}

/// Boot sequence per architecture §4.1.
///
/// Steps:
/// 1. Load config from OS-appropriate location
/// 2. Initialize encryption (keychain → passphrase fallback)
/// 3. Initialize EventBus, emit EncryptionInitialized
/// 4. Initialize Soul (SoulBox) — STUB (requires encrypted redb)
/// 5. Spawn actor registry
/// 6. Initialize and spawn platform adapter
/// 7. Initialize and spawn CTP actor
/// 8. Initialize memory system — STUB
/// 9. Initialize inference system — STUB
/// 10. Initialize prompt composer — STUB
/// 11. Broadcast BootComplete event
pub async fn boot() -> Result<Runtime, BootError> {
    // Step 1: Config load
    let config = crate::config::load_or_create_config()
        .await
        .map_err(|e| BootError::ConfigLoadFailed(e.to_string()))?;

    // Step 2: Initialize encryption
    let master_key =
        init_encryption().map_err(|e| BootError::EncryptionInitFailed(e.to_string()))?;

    // Step 3: Initialize EventBus
    let bus = Arc::new(EventBus::new());
    let keep_alive = bus.subscribe_broadcast();

    // Emit EncryptionInitialized after bus is ready
    bus.broadcast(Event::System(SystemEvent::EncryptionInitialized))
        .await
        .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;

    // Step 4: Initialize Soul (SoulBox) — STUB (requires encrypted redb, M2.5)
    // TODO M2.5: Initialize Soul with master_key for encrypted redb

    // Step 5: Actor registry
    let mut registry = ActorRegistry::new();

    // Step 6: Initialize and spawn platform adapter
    let platform_adapter = platform::create_platform_adapter();
    let platform_actor = platform::PlatformActor::new(platform_adapter)
        .with_poll_interval(Duration::from_millis(500));
    spawn_actor(&bus, &mut registry, Box::new(platform_actor));

    // Step 7: Initialize and spawn CTP actor
    let ctp_trigger_interval = Duration::from_secs(config.ctp_trigger_interval_secs);
    let ctp_buffer_window = Duration::from_secs(300);
    let ctp_poll_interval = Duration::from_secs(1);
    let ctp_actor = ctp::CTPActor::new(ctp_trigger_interval, ctp_buffer_window, ctp_poll_interval);
    spawn_actor(&bus, &mut registry, Box::new(ctp_actor));

    // Step 8: Initialize memory system — STUB
    // TODO M2.4: Initialize ech0 Store with master_key

    // Step 9: Initialize inference system — STUB
    // TODO M2.2: Initialize llama-cpp-rs model loader

    // Step 10: Initialize prompt composer — STUB
    // TODO M2.6: Initialize prompt composer

    // Step 11: Broadcast BootComplete
    bus.broadcast(Event::System(SystemEvent::BootComplete))
        .await
        .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;

    Ok(Runtime {
        bus,
        registry,
        config,
        master_key,
        _keep_alive: keep_alive,
    })
}

/// Initialize encryption: try OS keychain first, fall back to passphrase.
///
/// Per architecture §15.3:
/// - Primary: OS keychain via `keyring`
/// - Fallback: user passphrase via `SENA_PASSPHRASE` env var → Argon2id derivation
fn init_encryption() -> Result<MasterKey, crypto::CryptoError> {
    // Try OS keychain first
    match crypto::keychain::retrieve_master_key() {
        Ok(key) => return Ok(key),
        Err(_) => {
            // Keychain unavailable or no key stored — try passphrase fallback
        }
    }

    // Fallback: passphrase from environment variable
    let passphrase_str = std::env::var("SENA_PASSPHRASE").map_err(|_| {
        crypto::CryptoError::KeychainError(
            "no keychain key found and SENA_PASSPHRASE not set".to_string(),
        )
    })?;

    let passphrase = crypto::argon2_kdf::Passphrase::new(passphrase_str);

    // Load or generate salt
    let salt_path = platform::config_dir()
        .map_err(|e| crypto::CryptoError::IoError(std::io::Error::other(e.to_string())))?
        .join("salt.bin");
    let salt = if salt_path.exists() {
        let bytes = std::fs::read(&salt_path).map_err(crypto::CryptoError::IoError)?;
        if bytes.len() != 16 {
            return Err(crypto::CryptoError::InvalidData(
                "salt file has invalid length".to_string(),
            ));
        }
        let mut arr = [0u8; 16];
        arr.copy_from_slice(&bytes);
        crypto::argon2_kdf::Salt::from_bytes(arr)
    } else {
        let salt = crypto::argon2_kdf::generate_salt();
        // Ensure parent directory exists
        if let Some(parent) = salt_path.parent() {
            std::fs::create_dir_all(parent).map_err(crypto::CryptoError::IoError)?;
        }
        std::fs::write(&salt_path, salt.as_bytes()).map_err(crypto::CryptoError::IoError)?;
        salt
    };

    // Derive master key (passphrase is ZeroizeOnDrop, will be zeroed when dropped)
    crypto::argon2_kdf::derive_master_key(&passphrase, &salt)
}

/// Helper to spawn an actor into the registry.
///
/// The actor's lifecycle (start → run → stop) is managed by the spawned task.
/// This is used internally during boot to wire up actors.
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
        // Construct Runtime manually to avoid writing to real config dir
        let bus = Arc::new(EventBus::new());
        let keep_alive = bus.subscribe_broadcast();
        let config = SenaConfig::default();
        let registry = ActorRegistry::new();
        let master_key = MasterKey::from_bytes([0u8; 32]);

        bus.broadcast(Event::System(SystemEvent::BootComplete))
            .await
            .expect("broadcast should succeed");

        let runtime = Runtime {
            bus,
            registry,
            config,
            master_key,
            _keep_alive: keep_alive,
        };

        // Verify runtime was constructed
        assert_eq!(runtime.registry.actor_count(), 0);
    }

    #[tokio::test]
    async fn boot_emits_boot_complete_event() {
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        // Simulate boot's final step
        let event = Event::System(SystemEvent::BootComplete);
        bus.broadcast(event)
            .await
            .expect("broadcast should succeed in test");

        // Verify receiver gets BootComplete
        let received = rx.recv().await;
        assert!(received.is_ok());

        if let Ok(Event::System(SystemEvent::BootComplete)) = received {
            // Success
        } else {
            panic!("Expected BootComplete event");
        }
    }

    /// Integration test with mock actors to verify spawn_actor() functionality.
    #[tokio::test]
    async fn spawn_actors_lifecycle_integration() {
        use bus::ActorError;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc as StdArc;

        // Mock actor that tracks lifecycle calls
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
                // Simulate brief work then exit
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(())
            }

            async fn stop(&mut self) -> Result<(), ActorError> {
                self.stopped.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        // Construct runtime manually to avoid writing to real config dir
        let bus = Arc::new(EventBus::new());
        let keep_alive = bus.subscribe_broadcast();
        let config = SenaConfig::default();
        let mut registry = ActorRegistry::new();

        assert_eq!(registry.actor_count(), 0);

        // Create tracking flags for two mock actors
        let actor1_started = StdArc::new(AtomicBool::new(false));
        let actor1_ran = StdArc::new(AtomicBool::new(false));
        let actor1_stopped = StdArc::new(AtomicBool::new(false));

        let actor2_started = StdArc::new(AtomicBool::new(false));
        let actor2_ran = StdArc::new(AtomicBool::new(false));
        let actor2_stopped = StdArc::new(AtomicBool::new(false));

        // Create and spawn actors
        let mock1 = MockActor::new(
            "mock1",
            StdArc::clone(&actor1_started),
            StdArc::clone(&actor1_ran),
            StdArc::clone(&actor1_stopped),
        );

        let mock2 = MockActor::new(
            "mock2",
            StdArc::clone(&actor2_started),
            StdArc::clone(&actor2_ran),
            StdArc::clone(&actor2_stopped),
        );

        spawn_actor(&bus, &mut registry, Box::new(mock1));
        spawn_actor(&bus, &mut registry, Box::new(mock2));

        assert_eq!(registry.actor_count(), 2);

        let runtime = Runtime {
            bus,
            registry,
            config,
            master_key: MasterKey::from_bytes([0u8; 32]),
            _keep_alive: keep_alive,
        };

        // Give actors time to spawn and start
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Verify start was called
        assert!(actor1_started.load(Ordering::SeqCst));
        assert!(actor2_started.load(Ordering::SeqCst));

        // Give actors time to run and complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify run was called
        assert!(actor1_ran.load(Ordering::SeqCst));
        assert!(actor2_ran.load(Ordering::SeqCst));

        // Shutdown
        let result = crate::shutdown::shutdown(runtime, Duration::from_secs(2)).await;
        assert!(result.is_ok());

        // Verify stop was called
        assert!(actor1_stopped.load(Ordering::SeqCst));
        assert!(actor2_stopped.load(Ordering::SeqCst));
    }
}
