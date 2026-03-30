//! Placeholder ech0 types until real Git dependency is available.
//!
//! TODO: Replace this entire module with:
//!   ech0 = { git = "https://github.com/<org>/ech0", features = ["full"] }
//!
//! This placeholder provides minimal types to allow Phase 2 integration
//! to compile and demonstrate the architecture.

use std::path::PathBuf;

/// Placeholder for ech0's Store type.
pub struct Store {
    _graph_path: PathBuf,
    _vector_path: PathBuf,
}

impl Store {
    pub fn new(_config: StorePathConfig, _embedder: impl Embedder, _extractor: impl Extractor) -> Result<Self, String> {
        Ok(Self {
            _graph_path: PathBuf::new(),
            _vector_path: PathBuf::new(),
        })
    }

    pub async fn ingest_text(&self, _text: &str) -> Result<IngestResult, String> {
        Ok(IngestResult {
            node_id: 1,
            conflict: None,
        })
    }

    pub async fn search(&self, _query: &str, _opts: SearchOptions) -> Result<Vec<ScoredNode>, String> {
        Ok(vec![])
    }
}

/// Placeholder for ech0's StorePathConfig.
pub struct StorePathConfig {
    pub graph_path: PathBuf,
    pub vector_path: PathBuf,
}

/// Placeholder for ech0's Embedder trait.
pub trait Embedder: Send + Sync + 'static {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
}

/// Placeholder for ech0's Extractor trait.
pub trait Extractor: Send + Sync + 'static {
    fn extract(&self, text: &str) -> Result<Vec<String>, String>;
}

/// Placeholder for ingest result.
pub struct IngestResult {
    pub node_id: u64,
    pub conflict: Option<ConflictReport>,
}

/// Placeholder for conflict report.
pub struct ConflictReport {
    pub description: String,
    pub conflicting_node_id: u64,
}

/// Placeholder for search options.
pub struct SearchOptions {
    pub tier: SearchTier,
    pub max_results: usize,
}

/// Placeholder for search tier.
pub enum SearchTier {
    Graph,
    Vector,
}

/// Placeholder for scored node.
pub struct ScoredNode {
    pub node_id: u64,
    pub text: String,
    pub score: f32,
}
