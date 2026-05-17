use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{FunctionStubStatus, HealthStatus, RegisteredSriNode, SriTreeNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeAction {
    Open,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalSource {
    Identity,
    Perception,
    Cognition,
    Expression,
    Environment,
    Fault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    Ram,
    Cpu,
    Vram,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorResourceEstimate {
    pub actor_name: String,
    pub ram_mb: u64,
    pub cpu_pct: f32,
    pub vram_pct: Option<f32>,
    pub basis: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriResourceSnapshot {
    pub total_ram_mb: u64,
    pub total_cpu_pct: f32,
    pub vram_used_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
    pub actors: Vec<ActorResourceEstimate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SriSnapshot {
    pub tree: SriTreeNode,
    pub nodes: Vec<RegisteredSriNode>,
    pub open_shelves: Vec<String>,
    pub latest_resources: Option<SriResourceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SriEvent {
    TreeSnapshot {
        tree: SriTreeNode,
    },
    TreeNavigation {
        shelf_path: String,
        action: TreeAction,
        triggered_by: String,
    },
    SignalReceived {
        source: SignalSource,
        summary: String,
        timestamp: DateTime<Utc>,
    },
    NodeHealthChanged {
        shelf_path: String,
        old: HealthStatus,
        new: HealthStatus,
    },
    FunctionCallStub {
        path: String,
        status: FunctionStubStatus,
    },
    ResourceSnapshot(SriResourceSnapshot),
    ResourceAlert {
        actor: String,
        resource: ResourceKind,
        value: f32,
        threshold: f32,
    },
}