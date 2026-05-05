//! Inference work queue — typed work items with priority ordering.
//!
//! The `InferenceQueue` maintains three priority buckets (High, Normal, Low).
//! `dequeue()` always returns the highest-priority item available, preserving
//! FIFO order within each priority level.

use bus::events::ctp::EnrichedInferredTask;
use bus::{CausalId, ContextInterpretationInput, InferenceSource, Priority};
use std::collections::VecDeque;
use tokio::sync::oneshot;

/// A single unit of inference work.
pub struct WorkItem {
    /// Scheduling priority for this work item.
    pub priority: Priority,
    /// The type of work to perform.
    pub kind: WorkKind,
}

impl std::fmt::Debug for WorkItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkItem")
            .field("priority", &self.priority)
            .field("kind", &self.kind)
            .finish()
    }
}

impl std::fmt::Debug for WorkKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkKind::Inference {
                source, causal_id, ..
            } => f
                .debug_struct("Inference")
                .field("source", source)
                .field("causal_id", causal_id)
                .finish(),
            WorkKind::Embed { causal_id, .. } => f
                .debug_struct("Embed")
                .field("causal_id", causal_id)
                .finish(),
            WorkKind::Extract { causal_id, .. } => f
                .debug_struct("Extract")
                .field("causal_id", causal_id)
                .finish(),
            WorkKind::InterpretContext { causal_id, .. } => f
                .debug_struct("InterpretContext")
                .field("causal_id", causal_id)
                .finish(),
        }
    }
}

/// The concrete work task to execute.
// allowed: boxing the large context payload would add heap indirection and
// reshape the internal worker API during this style-only cleanup.
#[allow(clippy::large_enum_variant)]
pub enum WorkKind {
    /// Text generation from a prompt.
    Inference {
        prompt: String,
        source: InferenceSource,
        causal_id: CausalId,
        /// Optional channel to return the full generated text to a direct caller.
        response_tx: Option<oneshot::Sender<Result<String, String>>>,
    },
    /// Embedding vector generation.
    Embed {
        text: String,
        causal_id: CausalId,
        response_tx: oneshot::Sender<Result<Vec<f32>, String>>,
    },
    /// Structured fact extraction.
    Extract {
        text: String,
        causal_id: CausalId,
        response_tx: oneshot::Sender<Result<String, String>>,
    },
    /// Model-driven interpretation of a CTP context snapshot.
    InterpretContext {
        context: ContextInterpretationInput,
        causal_id: CausalId,
        response_tx: oneshot::Sender<Result<Option<EnrichedInferredTask>, String>>,
    },
}

/// Priority-ordered, capacity-bounded inference work queue.
///
/// Items are organized into three buckets by priority level.
/// `dequeue()` drains High before Normal before Low.
pub struct InferenceQueue {
    high: VecDeque<WorkItem>,
    normal: VecDeque<WorkItem>,
    low: VecDeque<WorkItem>,
    capacity: usize,
}

impl InferenceQueue {
    /// Create a new queue with the given total capacity across all priority levels.
    pub fn new(capacity: usize) -> Self {
        Self {
            high: VecDeque::new(),
            normal: VecDeque::new(),
            low: VecDeque::new(),
            capacity,
        }
    }

    /// Enqueue a work item.
    ///
    /// Returns `Err(item)` (the rejected item) if the queue is at capacity.
    // allowed: returning the rejected `WorkItem` preserves the existing
    // ownership handoff without introducing heap allocation or wider API churn.
    #[allow(clippy::result_large_err)]
    pub fn enqueue(&mut self, item: WorkItem) -> Result<(), WorkItem> {
        if self.len() >= self.capacity {
            return Err(item);
        }
        match item.priority {
            Priority::High => self.high.push_back(item),
            Priority::Normal => self.normal.push_back(item),
            Priority::Low => self.low.push_back(item),
        }
        Ok(())
    }

    /// Remove and return the next highest-priority item, or `None` if empty.
    pub fn dequeue(&mut self) -> Option<WorkItem> {
        if let Some(item) = self.high.pop_front() {
            return Some(item);
        }
        if let Some(item) = self.normal.pop_front() {
            return Some(item);
        }
        self.low.pop_front()
    }

    /// Total number of items currently in the queue across all priority levels.
    pub fn len(&self) -> usize {
        self.high.len() + self.normal.len() + self.low.len()
    }

    /// Return `true` if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(priority: Priority) -> WorkItem {
        WorkItem {
            priority,
            kind: WorkKind::Inference {
                prompt: "test".to_string(),
                source: InferenceSource::UserText,
                causal_id: CausalId::new(),
                response_tx: None,
            },
        }
    }

    #[test]
    fn queue_dequeues_high_before_normal() {
        let mut q = InferenceQueue::new(10);
        q.enqueue(make_item(Priority::Normal))
            .expect("enqueue normal");
        q.enqueue(make_item(Priority::High)).expect("enqueue high");
        let item = q.dequeue().expect("dequeue should return item");
        assert_eq!(item.priority, Priority::High);
    }

    #[test]
    fn queue_dequeues_normal_before_low() {
        let mut q = InferenceQueue::new(10);
        q.enqueue(make_item(Priority::Low)).expect("enqueue low");
        q.enqueue(make_item(Priority::Normal))
            .expect("enqueue normal");
        let item = q.dequeue().expect("dequeue should return item");
        assert_eq!(item.priority, Priority::Normal);
    }

    #[test]
    fn queue_rejects_when_full() {
        let mut q = InferenceQueue::new(1);
        q.enqueue(make_item(Priority::Normal))
            .expect("first enqueue should succeed");
        let result = q.enqueue(make_item(Priority::Normal));
        assert!(
            result.is_err(),
            "second enqueue should fail when at capacity"
        );
    }

    #[test]
    fn queue_len_is_correct() {
        let mut q = InferenceQueue::new(10);
        assert_eq!(q.len(), 0);
        q.enqueue(make_item(Priority::High)).expect("enqueue");
        q.enqueue(make_item(Priority::Normal)).expect("enqueue");
        assert_eq!(q.len(), 2);
        q.dequeue();
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn queue_empty_dequeue_returns_none() {
        let mut q = InferenceQueue::new(10);
        assert!(q.dequeue().is_none());
    }
}
