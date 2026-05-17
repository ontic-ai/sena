use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Active,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionStubStatus {
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionStub {
    pub name: String,
    pub description: String,
    pub status: FunctionStubStatus,
}

impl FunctionStub {
    pub fn unavailable(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            status: FunctionStubStatus::Unavailable,
        }
    }
}

pub trait SriNode: Send + Sync {
    fn shelf_path(&self) -> &str;

    fn display_name(&self) -> &str;

    fn description(&self) -> &str;

    fn function_stubs(&self) -> Vec<FunctionStub>;

    fn health_status(&self) -> HealthStatus {
        HealthStatus::Unavailable
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredSriNode {
    pub shelf_path: String,
    pub display_name: String,
    pub description: String,
    pub function_stubs: Vec<FunctionStub>,
    pub health_status: HealthStatus,
}

impl RegisteredSriNode {
    pub fn from_node(node: &dyn SriNode) -> Self {
        let shelf_path = normalize_shelf_path(node.shelf_path());
        assert!(!shelf_path.is_empty(), "SRI shelf_path cannot be empty");

        Self {
            shelf_path,
            display_name: node.display_name().trim().to_string(),
            description: node.description().trim().to_string(),
            function_stubs: node.function_stubs(),
            health_status: node.health_status(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SriTreeNode {
    pub shelf_path: String,
    pub display_name: String,
    pub description: String,
    pub registered: bool,
    pub function_stubs: Vec<FunctionStub>,
    pub health_status: HealthStatus,
    pub children: Vec<SriTreeNode>,
}

impl SriTreeNode {
    pub fn root() -> Self {
        Self {
            shelf_path: String::new(),
            display_name: "SELF".to_string(),
            description: String::new(),
            registered: false,
            function_stubs: Vec::new(),
            health_status: HealthStatus::Unavailable,
            children: Vec::new(),
        }
    }

    pub(crate) fn structural(
        shelf_path: String,
        display_name: String,
        children: Vec<SriTreeNode>,
        health_status: HealthStatus,
    ) -> Self {
        Self {
            shelf_path,
            display_name,
            description: String::new(),
            registered: false,
            function_stubs: Vec::new(),
            health_status,
            children,
        }
    }
}

pub(crate) fn normalize_shelf_path(path: &str) -> String {
    path.split('.')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}