//! Background loop registry and control commands.

use async_trait::async_trait;
use bus::{Event, EventBus, SystemEvent};
use ipc::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Metadata about a background loop.
#[derive(Debug, Clone)]
pub struct LoopMetadata {
    /// Canonical loop name (lowercase, underscore-separated).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Default enabled state.
    pub default_enabled: bool,
    /// Current enabled state.
    pub enabled: bool,
}

/// Loop registry: tracks all known background loops and their current state.
#[derive(Clone)]
pub struct LoopRegistry {
    loops: Arc<RwLock<HashMap<String, LoopMetadata>>>,
}

impl LoopRegistry {
    /// Create a new loop registry with canonical Sena loops.
    pub fn new() -> Self {
        let mut loops = HashMap::new();

        // CTP loop
        loops.insert(
            "ctp".to_string(),
            LoopMetadata {
                name: "ctp".to_string(),
                description: "Continuous thought processing — signal ingestion and proactive inference trigger".to_string(),
                default_enabled: true,
                enabled: true,
            },
        );

        // Memory consolidation loop
        loops.insert(
            "memory_consolidation".to_string(),
            LoopMetadata {
                name: "memory_consolidation".to_string(),
                description:
                    "Periodic memory consolidation — moves working memory to long-term store"
                        .to_string(),
                default_enabled: true,
                enabled: true,
            },
        );

        // Platform polling loop
        loops.insert(
            "platform_polling".to_string(),
            LoopMetadata {
                name: "platform_polling".to_string(),
                description:
                    "Platform signal polling — active window, clipboard, keystroke cadence"
                        .to_string(),
                default_enabled: true,
                enabled: true,
            },
        );

        // Screen capture loop
        loops.insert(
            "screen_capture".to_string(),
            LoopMetadata {
                name: "screen_capture".to_string(),
                description:
                    "Screen capture for vision-capable models — periodic screenshot acquisition"
                        .to_string(),
                default_enabled: true,
                enabled: true,
            },
        );

        // Speech input loop
        loops.insert(
            "speech".to_string(),
            LoopMetadata {
                name: "speech".to_string(),
                description: "Speech input loop — wakeword detection and/or continuous STT capture"
                    .to_string(),
                default_enabled: true,
                enabled: true,
            },
        );

        // VRAM monitor loop
        loops.insert(
            "vram_monitor".to_string(),
            LoopMetadata {
                name: "vram_monitor".to_string(),
                description: "Real-time VRAM usage monitoring — polls GPU memory every 10s"
                    .to_string(),
                default_enabled: true,
                enabled: true,
            },
        );

        Self {
            loops: Arc::new(RwLock::new(loops)),
        }
    }

    /// List all registered loops.
    pub async fn list(&self) -> Vec<LoopMetadata> {
        let loops = self.loops.read().await;
        let mut result: Vec<LoopMetadata> = loops.values().cloned().collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// Get metadata for a specific loop.
    pub async fn get(&self, name: &str) -> Option<LoopMetadata> {
        self.loops.read().await.get(name).cloned()
    }

    /// Update the enabled state of a loop.
    ///
    /// Returns true if the loop was found and updated, false otherwise.
    #[allow(dead_code)] // Reserved for future use; registry updates now come from LoopStatusChanged events
    pub async fn set_enabled(&self, name: &str, enabled: bool) -> bool {
        let mut loops = self.loops.write().await;
        if let Some(metadata) = loops.get_mut(name) {
            metadata.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Handle a LoopStatusChanged event from an actor.
    ///
    /// Updates the registry to reflect the actual loop state.
    pub async fn handle_status_changed(&self, loop_name: &str, enabled: bool) {
        let mut loops = self.loops.write().await;
        if let Some(metadata) = loops.get_mut(loop_name) {
            metadata.enabled = enabled;
        }
    }
}

impl Default for LoopRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Handler for "loops.list" command.
pub struct LoopsListHandler {
    registry: LoopRegistry,
}

impl LoopsListHandler {
    pub fn new(registry: LoopRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl CommandHandler for LoopsListHandler {
    fn name(&self) -> &'static str {
        "loops.list"
    }

    fn description(&self) -> &'static str {
        "List all registered background loops and their current state"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        let loops = self.registry.list().await;
        let loops_json: Vec<Value> = loops
            .iter()
            .map(|l| {
                json!({
                    "name": l.name,
                    "description": l.description,
                    "default_enabled": l.default_enabled,
                    "enabled": l.enabled,
                })
            })
            .collect();
        Ok(json!({ "loops": loops_json }))
    }
}

/// Handler for "loops.set" command.
pub struct LoopsSetHandler {
    registry: LoopRegistry,
    bus: Arc<EventBus>,
}

impl LoopsSetHandler {
    pub fn new(registry: LoopRegistry, bus: Arc<EventBus>) -> Self {
        Self { registry, bus }
    }
}

#[async_trait]
impl CommandHandler for LoopsSetHandler {
    fn name(&self) -> &'static str {
        "loops.set"
    }

    fn description(&self) -> &'static str {
        "Enable or disable a background loop"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
        let loop_name = payload
            .get("loop_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| IpcError::InvalidPayload("missing loop_name".to_string()))?
            .to_string();

        let enabled = payload
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| IpcError::InvalidPayload("missing enabled".to_string()))?;

        // Check if loop exists
        if self.registry.get(&loop_name).await.is_none() {
            return Err(IpcError::CommandFailed(format!(
                "Unknown loop: {}",
                loop_name
            )));
        }

        // Broadcast LoopControlRequested event — actor will respond with LoopStatusChanged
        // Only actual loop owners respond to this event
        self.bus
            .broadcast(Event::System(SystemEvent::LoopControlRequested {
                loop_name: loop_name.clone(),
                enabled,
            }))
            .await
            .map_err(|e| IpcError::CommandFailed(e.to_string()))?;

        Ok(json!({
            "loop_name": loop_name,
            "enabled": enabled,
            "status": "requested"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_initializes_with_canonical_loops() {
        let registry = LoopRegistry::new();
        let loops = registry.list().await;

        assert_eq!(loops.len(), 6);

        let loop_names: Vec<&str> = loops.iter().map(|l| l.name.as_str()).collect();
        assert!(loop_names.contains(&"ctp"));
        assert!(loop_names.contains(&"memory_consolidation"));
        assert!(loop_names.contains(&"platform_polling"));
        assert!(loop_names.contains(&"screen_capture"));
        assert!(loop_names.contains(&"speech"));
        assert!(loop_names.contains(&"vram_monitor"));
    }

    #[tokio::test]
    async fn registry_get_returns_metadata() {
        let registry = LoopRegistry::new();
        let ctp = registry.get("ctp").await.unwrap();

        assert_eq!(ctp.name, "ctp");
        assert!(ctp.default_enabled);
        assert!(ctp.enabled);
    }

    #[tokio::test]
    async fn registry_set_enabled_updates_state() {
        let registry = LoopRegistry::new();

        assert!(registry.set_enabled("ctp", false).await);

        let ctp = registry.get("ctp").await.unwrap();
        assert!(!ctp.enabled);
    }

    #[tokio::test]
    async fn registry_set_enabled_returns_false_for_unknown_loop() {
        let registry = LoopRegistry::new();
        assert!(!registry.set_enabled("nonexistent", false).await);
    }

    #[tokio::test]
    async fn registry_handle_status_changed_updates_state() {
        let registry = LoopRegistry::new();

        registry.handle_status_changed("ctp", false).await;

        let ctp = registry.get("ctp").await.unwrap();
        assert!(!ctp.enabled);
    }
}
