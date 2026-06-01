use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HealthStatus {
    #[default]
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NodeState {
    #[default]
    IndexedOut,
    IndexedIn,
    Active,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SriNode {
    pub id: String,
    pub label: String,
    pub parent_id: Option<String>,
    pub health: HealthStatus,
    pub state: NodeState,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SriDelta {
    Registered { id: String },
    HealthChanged { id: String, health: HealthStatus },
    StateChanged { id: String, state: NodeState },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SriSnapshot {
    pub active_root: Option<String>,
    pub nodes: Vec<SriNode>,
    pub delta: Vec<SriDelta>,
}

#[derive(Debug, Default)]
pub struct SriRegistry {
    nodes: BTreeMap<String, SriNode>,
    delta_log: Vec<SriDelta>,
}

impl SriRegistry {
    pub fn register(&mut self, node: SriNode) {
        self.delta_log.push(SriDelta::Registered {
            id: node.id.clone(),
        });
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn set_health(&mut self, id: &str, health: HealthStatus) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.health = health;
            self.delta_log.push(SriDelta::HealthChanged {
                id: id.to_owned(),
                health,
            });
        }
    }

    pub fn set_state(&mut self, id: &str, state: NodeState) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.state = state;
            self.delta_log.push(SriDelta::StateChanged {
                id: id.to_owned(),
                state,
            });
        }
    }

    pub fn snapshot(&self, active_root: Option<&str>) -> SriSnapshot {
        let nodes = match active_root {
            Some(root) => self
                .nodes
                .values()
                .filter(|node| node.id == root || self.is_descendant(node, root))
                .cloned()
                .collect(),
            None => self.nodes.values().cloned().collect(),
        };

        SriSnapshot {
            active_root: active_root.map(str::to_owned),
            nodes,
            delta: self.delta_log.clone(),
        }
    }

    fn is_descendant(&self, node: &SriNode, root: &str) -> bool {
        let mut cursor = node.parent_id.as_deref();
        while let Some(parent_id) = cursor {
            if parent_id == root {
                return true;
            }
            cursor = self.nodes.get(parent_id).and_then(|parent| parent.parent_id.as_deref());
        }
        false
    }
}
