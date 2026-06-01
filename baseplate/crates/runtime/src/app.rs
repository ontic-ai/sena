use std::{collections::BTreeMap, path::{Path, PathBuf}};

use bus::{BootProgressEvent, ConversationEvent, RuntimeEvent};
use sri::{HealthStatus, NodeState, SriNode, SriRegistry};

use crate::{
    config::RuntimeConfig,
    model::{
        boot_verification_plans, ensure_boot_inventory, resolve_sena_dir, verify_or_download_plan,
        BootInventory, BootModelPlans, ModelBootError, VerificationPlan,
    },
    protocol::{interpret_model_output, ProtocolMessage},
};

#[derive(Debug)]
pub struct SenaRuntime {
    config: RuntimeConfig,
    sri_registry: SriRegistry,
}

impl SenaRuntime {
    pub fn new(config: RuntimeConfig) -> Self {
        let mut sri_registry = SriRegistry::default();
        for node in seed_nodes() {
            sri_registry.register(node);
        }
        Self {
            config,
            sri_registry,
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn sri_registry(&self) -> &SriRegistry {
        &self.sri_registry
    }

    pub fn qwen_verification_plan(&self, cache_root: &Path) -> VerificationPlan {
        verify_or_download_plan(cache_root)
    }

    pub fn boot_model_plans(&self, cache_root: &Path) -> BootModelPlans {
        boot_verification_plans(cache_root)
    }

    pub fn resolve_data_dir(&self) -> Result<PathBuf, ModelBootError> {
        resolve_sena_dir()
    }

    pub async fn ensure_boot_inventory<F>(
        &self,
        cache_root: &Path,
        reporter: F,
    ) -> Result<BootInventory, ModelBootError>
    where
        F: FnMut(BootProgressEvent),
    {
        ensure_boot_inventory(cache_root, reporter).await
    }

    pub fn ingest_model_output(&self, raw: &str) -> RuntimeEvent {
        match interpret_model_output(raw) {
            ProtocolMessage::Parsed(output) => {
                RuntimeEvent::Conversation(ConversationEvent::ModelOutputParsed(output))
            }
            ProtocolMessage::Fallback { raw, reply, reason } => {
                RuntimeEvent::Conversation(ConversationEvent::ModelOutputFallback { raw, reply, reason })
            }
        }
    }
}

fn seed_nodes() -> Vec<SriNode> {
    vec![
        SriNode {
            id: "perception.hearing".to_owned(),
            label: "Perception / Hearing".to_owned(),
            parent_id: None,
            health: HealthStatus::Healthy,
            state: NodeState::IndexedIn,
            metadata: BTreeMap::new(),
        },
        SriNode {
            id: "expression.voice".to_owned(),
            label: "Expression / Voice".to_owned(),
            parent_id: None,
            health: HealthStatus::Healthy,
            state: NodeState::IndexedIn,
            metadata: BTreeMap::new(),
        },
        SriNode {
            id: "expression.language".to_owned(),
            label: "Expression / Language".to_owned(),
            parent_id: None,
            health: HealthStatus::Healthy,
            state: NodeState::IndexedIn,
            metadata: BTreeMap::new(),
        },
        SriNode {
            id: "cognition.capabilities".to_owned(),
            label: "Cognition / Capabilities".to_owned(),
            parent_id: None,
            health: HealthStatus::Healthy,
            state: NodeState::Active,
            metadata: BTreeMap::new(),
        },
        SriNode {
            id: "environment.runtime".to_owned(),
            label: "Environment / Runtime".to_owned(),
            parent_id: None,
            health: HealthStatus::Healthy,
            state: NodeState::Active,
            metadata: BTreeMap::new(),
        },
        SriNode {
            id: "actions".to_owned(),
            label: "Actions".to_owned(),
            parent_id: None,
            health: HealthStatus::Degraded,
            state: NodeState::IndexedOut,
            metadata: BTreeMap::new(),
        },
    ]
}

