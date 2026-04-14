//! Causal ID for event chain tracking.

use std::sync::atomic::{AtomicU64, Ordering};

static CAUSAL_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Unique identifier for causal event chains.
///
/// Used to track request-response chains across the bus.
/// Events that participate in a chain (e.g., InferenceRequested → InferenceCompleted)
/// carry the same CausalId for correlation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CausalId(u64);

impl CausalId {
    /// Generate a new unique causal ID.
    pub fn new() -> Self {
        Self(CAUSAL_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// A sentinel value representing no causal chain.
    pub fn none() -> Self {
        Self(0)
    }

    /// Returns true if this ID is the sentinel value.
    pub fn is_none(&self) -> bool {
        self.0 == 0
    }

    /// Get the raw u64 value.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl Default for CausalId {
    fn default() -> Self {
        Self::none()
    }
}

impl From<u64> for CausalId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl serde::Serialize for CausalId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for CausalId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = u64::deserialize(deserializer)?;
        Ok(Self(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causal_id_generates_unique_values() {
        let id1 = CausalId::new();
        let id2 = CausalId::new();
        assert_ne!(id1, id2);
        assert!(id2.as_u64() > id1.as_u64());
    }

    #[test]
    fn causal_id_none_is_zero() {
        let none = CausalId::none();
        assert_eq!(none.as_u64(), 0);
        assert!(none.is_none());
    }

    #[test]
    fn causal_id_new_is_not_none() {
        let id = CausalId::new();
        assert!(!id.is_none());
    }

    #[test]
    fn causal_id_from_u64() {
        let id = CausalId::from(42);
        assert_eq!(id.as_u64(), 42);
    }

    #[test]
    fn causal_id_default_is_none() {
        let id = CausalId::default();
        assert!(id.is_none());
    }
}
