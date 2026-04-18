//! Soul schema definitions and version constant.
//!
//! `SchemaV1` is the in-memory personality/preference model maintained by
//! `SoulActor`. It is loaded from the encrypted store on startup and
//! updated incrementally as identity signals are written.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Current schema version. Incremented with each breaking schema change.
pub const SCHEMA_VERSION: u32 = 1;

/// Work cadence preference derived from behavioral observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorkCadence {
    /// Short bursts of intense activity.
    Burst,
    /// Consistent steady pace.
    #[default]
    Steady,
    /// Long uninterrupted focus sessions.
    LongFocus,
}

/// An entry in the window context history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowHistoryEntry {
    /// Application name.
    pub app_name: String,
    /// Window title fragment (optional, privacy-safe).
    pub window_title: Option<String>,
    /// When this entry was recorded.
    pub timestamp: SystemTime,
}

/// Schema version 1 — the in-memory personality and preference model.
///
/// Updated by `SoulActor` as identity signals and temporal patterns are collected.
/// Converted to `PersonalityMetadata` and broadcast on the bus after updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaV1 {
    /// Response verbosity preference: 0.0 (concise) to 1.0 (detailed).
    pub verbosity_preference: f64,
    /// Response warmth: 0.0 (formal) to 1.0 (warm/friendly).
    pub response_warmth: f64,
    /// Preferred work cadence derived from keystroke patterns.
    pub work_cadence_preference: WorkCadence,
    /// Recent application usage history.
    pub window_history: Vec<WindowHistoryEntry>,
    /// Schema version stamp.
    pub schema_version: u32,
}

impl Default for SchemaV1 {
    fn default() -> Self {
        Self {
            verbosity_preference: 0.5,
            response_warmth: 0.6,
            work_cadence_preference: WorkCadence::Steady,
            window_history: Vec::new(),
            schema_version: SCHEMA_VERSION,
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
        assert_eq!(schema.work_cadence_preference, WorkCadence::Steady);
    }

    #[test]
    fn schema_v1_is_serializable() {
        let schema = SchemaV1::default();
        let json = serde_json::to_string(&schema).expect("serialize should succeed");
        let restored: SchemaV1 = serde_json::from_str(&json).expect("deserialize should succeed");
        assert_eq!(restored.schema_version, SCHEMA_VERSION);
    }
}
