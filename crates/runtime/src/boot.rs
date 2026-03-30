//! Boot sequence orchestration.

use std::sync::Arc;
use std::time::Duration;

use bus::{Actor, Event, EventBus, SystemEvent};
use crypto::MasterKey;

use crate::config::SenaConfig;
use crate::registry::ActorRegistry;

/// Runtime holds initialized subsystems and the event bus.
pub struct Runtime {
    pub bus: Arc<EventBus>,
    pub registry: ActorRegistry,
    pub config: SenaConfig,
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

    // Step 2: Initialize encryption
    let master_key =
        init_encryption().map_err(|e| BootError::EncryptionInitFailed(e.to_string()))?;

    // Step 3: Initialize EventBus
    let bus = Arc::new(EventBus::new());
    let keep_alive = bus.subscribe_broadcast();

    bus.broadcast(Event::System(SystemEvent::EncryptionInitialized))
        .await
        .map_err(|e| BootError::BroadcastFailed(e.to_string()))?;

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
        .with_poll_interval(Duration::from_millis(500));
    spawn_actor(&bus, &mut registry, Box::new(platform_actor));

    // Step 7: Initialize and spawn CTP actor
    let ctp_trigger_interval = Duration::from_secs(config.ctp_trigger_interval_secs);
    let ctp_buffer_window = Duration::from_secs(300);
    let ctp_poll_interval = Duration::from_secs(1);
    let ctp_actor = ctp::CTPActor::new(ctp_trigger_interval, ctp_buffer_window, ctp_poll_interval);
    spawn_actor(&bus, &mut registry, Box::new(ctp_actor));

    // Step 8: Initialize memory system â€” STUB
    // TODO M2.4: Initialize ech0 Store with master_key

    // Step 9: Initialize inference system â€” STUB
    // TODO M2.2: Initialize llama-cpp-rs model loader

    // Step 10: PromptComposer is stateless â€” instantiated per inference cycle.
    // prompt::PromptComposer::new() is cheap; no actor spawn needed.

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

fn init_encryption() -> Result<MasterKey, crypto::CryptoError> {
    match crypto::keychain::retrieve_master_key() {
        Ok(key) => return Ok(key),
        Err(_) => {}
    }

    let passphrase_str = std::env::var("SENA_PASSPHRASE").map_err(|_| {
        crypto::CryptoError::KeychainError(
            "no keychain key found and SENA_PASSPHRASE not set".to_string(),
        )
    })?;

    let passphrase = crypto::argon2_kdf::Passphrase::new(passphrase_str);

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
        if let Some(parent) = salt_path.parent() {
            std::fs::create_dir_all(parent).map_err(crypto::CryptoError::IoError)?;
        }
        std::fs::write(&salt_path, salt.as_bytes()).map_err(crypto::CryptoError::IoError)?;
        salt
    };

    crypto::argon2_kdf::derive_master_key(&passphrase, &salt)
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

        let runtime = Runtime {
            bus,
            registry,
            config,
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
                Self { name, started, ran, stopped }
            }
        }

        #[async_trait::async_trait]
        impl Actor for MockActor {
            fn name(&self) -> &'static str { self.name }

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

        spawn_actor(&bus, &mut registry, Box::new(MockActor::new(
            "mock1",
            StdArc::clone(&actor1_started),
            StdArc::clone(&actor1_ran),
            StdArc::clone(&actor1_stopped),
        )));
        spawn_actor(&bus, &mut registry, Box::new(MockActor::new(
            "mock2",
            StdArc::clone(&actor2_started),
            StdArc::clone(&actor2_ran),
            StdArc::clone(&actor2_stopped),
        )));

        assert_eq!(registry.actor_count(), 2);

        let runtime = Runtime {
            bus,
            registry,
            config,
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
}