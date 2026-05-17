use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;

use crate::{HealthStatus, RegisteredSriNode, SriEvent, SriNode, SriTreeNode};

#[derive(Clone, Default)]
pub struct SriRegistry {
    inner: Arc<RwLock<RegistryState>>,
    notifier: Arc<RwLock<Option<broadcast::Sender<SriEvent>>>>,
}

#[derive(Default)]
struct RegistryState {
    nodes: BTreeMap<String, RegisteredSriNode>,
}

#[derive(Default)]
struct BuildNode {
    shelf_path: String,
    segment: String,
    registered: Option<RegisteredSriNode>,
    children: BTreeMap<String, BuildNode>,
}

impl SriRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attach_notifier(&self, notifier: broadcast::Sender<SriEvent>) {
        *self.notifier.write().expect("sri notifier lock poisoned") = Some(notifier);
    }

    pub fn register(&self, node: Box<dyn SriNode>) {
        let registered = RegisteredSriNode::from_node(node.as_ref());
        self.inner
            .write()
            .expect("sri registry lock poisoned")
            .nodes
            .insert(registered.shelf_path.clone(), registered);

        self.broadcast_tree_snapshot();
    }

    pub fn nodes(&self) -> Vec<RegisteredSriNode> {
        self.inner
            .read()
            .expect("sri registry lock poisoned")
            .nodes
            .values()
            .cloned()
            .collect()
    }

    pub fn set_health(
        &self,
        shelf_path: &str,
        new_status: HealthStatus,
    ) -> Option<(HealthStatus, HealthStatus)> {
        let mut inner = self.inner.write().expect("sri registry lock poisoned");
        let node = inner.nodes.get_mut(shelf_path)?;
        if node.health_status == new_status {
            return None;
        }

        let old_status = node.health_status;
        node.health_status = new_status;
        Some((old_status, new_status))
    }

    pub fn get_tree(&self) -> SriTreeNode {
        build_tree(self.nodes())
    }

    pub fn broadcast_tree_snapshot(&self) {
        let notifier = self
            .notifier
            .read()
            .expect("sri notifier lock poisoned")
            .clone();
        let Some(notifier) = notifier else {
            return;
        };

        let _ = notifier.send(SriEvent::TreeSnapshot {
            tree: self.get_tree(),
        });
    }
}

fn build_tree(nodes: Vec<RegisteredSriNode>) -> SriTreeNode {
    let mut root = BuildNode {
        shelf_path: String::new(),
        segment: String::new(),
        registered: None,
        children: BTreeMap::new(),
    };

    for node in nodes {
        let mut cursor = &mut root;
        let mut accumulated = String::new();

        for segment in node.shelf_path.split('.') {
            if !accumulated.is_empty() {
                accumulated.push('.');
            }
            accumulated.push_str(segment);

            cursor = cursor.children.entry(segment.to_string()).or_insert_with(|| BuildNode {
                shelf_path: accumulated.clone(),
                segment: segment.to_string(),
                registered: None,
                children: BTreeMap::new(),
            });
        }

        cursor.registered = Some(node);
    }

    finalize_node(root)
}

fn finalize_node(node: BuildNode) -> SriTreeNode {
    let children = node
        .children
        .into_values()
        .map(finalize_node)
        .collect::<Vec<_>>();

    if node.shelf_path.is_empty() {
        return SriTreeNode {
            health_status: aggregate_health(None, &children),
            children,
            ..SriTreeNode::root()
        };
    }

    if let Some(registered) = node.registered {
        let health_status = aggregate_health(Some(registered.health_status), &children);
        return SriTreeNode {
            shelf_path: registered.shelf_path,
            display_name: registered.display_name,
            description: registered.description,
            registered: true,
            function_stubs: registered.function_stubs,
            health_status,
            children,
        };
    }

    let health_status = aggregate_health(None, &children);
    SriTreeNode::structural(node.shelf_path, node.segment, children, health_status)
}

fn aggregate_health(own: Option<HealthStatus>, children: &[SriTreeNode]) -> HealthStatus {
    let mut saw_degraded = matches!(own, Some(HealthStatus::Degraded));
    if matches!(own, Some(HealthStatus::Active)) {
        return HealthStatus::Active;
    }

    for child in children {
        match child.health_status {
            HealthStatus::Active => return HealthStatus::Active,
            HealthStatus::Degraded => saw_degraded = true,
            HealthStatus::Unavailable => {}
        }
    }

    if saw_degraded {
        HealthStatus::Degraded
    } else {
        HealthStatus::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{FunctionStub, FunctionStubStatus};

    struct TestNode {
        shelf_path: &'static str,
        health: HealthStatus,
    }

    impl SriNode for TestNode {
        fn shelf_path(&self) -> &str {
            self.shelf_path
        }

        fn display_name(&self) -> &str {
            self.shelf_path.rsplit('.').next().unwrap_or(self.shelf_path)
        }

        fn description(&self) -> &str {
            "test node"
        }

        fn function_stubs(&self) -> Vec<FunctionStub> {
            vec![FunctionStub {
                name: "noop".to_string(),
                description: "noop".to_string(),
                status: FunctionStubStatus::Unavailable,
            }]
        }

        fn health_status(&self) -> HealthStatus {
            self.health
        }
    }

    #[test]
    fn registry_builds_self_assembling_tree() {
        let registry = SriRegistry::new();
        registry.register(Box::new(TestNode {
            shelf_path: "cognition.memory",
            health: HealthStatus::Active,
        }));
        registry.register(Box::new(TestNode {
            shelf_path: "expression.voice",
            health: HealthStatus::Unavailable,
        }));

        let tree = registry.get_tree();
        assert_eq!(tree.display_name, "SELF");
        assert_eq!(tree.children.len(), 2);
        assert_eq!(tree.children[0].shelf_path, "cognition");
        assert_eq!(tree.children[0].children[0].shelf_path, "cognition.memory");
        assert_eq!(tree.children[0].health_status, HealthStatus::Active);
    }

    #[test]
    fn set_health_updates_registered_node() {
        let registry = SriRegistry::new();
        registry.register(Box::new(TestNode {
            shelf_path: "identity.soul",
            health: HealthStatus::Unavailable,
        }));

        let changed = registry.set_health("identity.soul", HealthStatus::Degraded);
        assert_eq!(
            changed,
            Some((HealthStatus::Unavailable, HealthStatus::Degraded))
        );

        let tree = registry.get_tree();
        assert_eq!(tree.children[0].children[0].health_status, HealthStatus::Degraded);
    }
}