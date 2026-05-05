//! Soul schema definitions and version constant.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub use bus::events::soul::{Verbosity, Warmth, WorkCadence};

/// Current schema version. Incremented with each breaking schema change.
pub const SCHEMA_VERSION: u32 = 1;

/// An entry in the window context history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowEntry {
    /// Application name.
    pub app_name: String,
    /// Window title fragment (optional, privacy-safe).
    pub window_title: Option<String>,
    /// When this entry was recorded.
    pub timestamp: DateTime<Utc>,
}

/// Schema version 1 — the in-memory personality and preference model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaV1 {
    /// Response verbosity preference.
    pub verbosity: Verbosity,
    /// Response warmth.
    pub warmth: Warmth,
    /// Preferred work cadence derived from keystroke patterns.
    pub work_cadence: WorkCadence,
    /// Recent application usage history.
    pub active_window_history: VecDeque<WindowEntry>,
    /// Schema version stamp.
    pub schema_version: u32,
    /// Number of runtime sessions observed.
    pub session_count: u64,
    /// Most recent time Sena was active.
    pub last_active: Option<DateTime<Utc>>,
    /// When this Soul schema was first created.
    pub created_at: DateTime<Utc>,
    /// Total cumulative interaction minutes tracked by Soul.
    pub total_interaction_minutes: u64,
    /// Sena's own name.
    pub name: String,
}

impl Default for SchemaV1 {
    fn default() -> Self {
        Self {
            verbosity: Verbosity::Balanced,
            warmth: Warmth::Friendly,
            work_cadence: WorkCadence::Steady,
            active_window_history: VecDeque::new(),
            schema_version: SCHEMA_VERSION,
            session_count: 0,
            last_active: None,
            created_at: Utc::now(),
            total_interaction_minutes: 0,
            name: "Sena".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_v1_has_defaults() {
        let schema = SchemaV1::default();
        assert_eq!(schema.schema_version, SCHEMA_VERSION);
        assert_eq!(schema.work_cadence, WorkCadence::Steady);
        assert_eq!(schema.name, "Sena");
        assert_eq!(schema.session_count, 0);
    }

    #[test]
    fn schema_v1_is_serializable() {
        let schema = SchemaV1::default();
        let json = serde_json::to_string(&schema).expect("serialize should succeed");
        let restored: SchemaV1 = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(restored.schema_version, SCHEMA_VERSION);
        assert_eq!(restored.name, "Sena");
    }
}
