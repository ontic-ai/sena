//! Priority inference queue.
//!
//! Bounded, priority-ordered queue for inference, embedding, and extraction requests.

use bus::events::inference::Priority;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use tokio::sync::oneshot;

/// Error from queue operations.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    /// Queue is at capacity.
    #[error("queue full — capacity {0} reached")]
    Full(usize),
}

/// The kind of work to perform.
#[derive(Debug)]
pub enum WorkKind {
    /// Run text inference.
    Infer {
        prompt: String,
        response_tx: oneshot::Sender<InferResult>,
    },
    /// Generate embedding vector.
    Embed {
        text: String,
        response_tx: oneshot::Sender<EmbedResult>,
    },
    /// Extract structured facts.
    Extract {
        text: String,
        response_tx: oneshot::Sender<ExtractResult>,
    },
}

/// Result of an inference call.
pub type InferResult = Result<(String, usize), String>;

/// Result of an embedding call.
pub type EmbedResult = Result<Vec<f32>, String>;

/// Result of an extraction call.
pub type ExtractResult = Result<Vec<String>, String>;

/// A queued work item with priority and sequence number.
pub struct QueuedWork {
    pub priority: Priority,
    pub request_id: u64,
    pub kind: WorkKind,
    /// Insertion order for FIFO within same priority.
    sequence: u64,
}

impl std::fmt::Debug for QueuedWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedWork")
            .field("priority", &self.priority)
            .field("request_id", &self.request_id)
            .field("sequence", &self.sequence)
            .finish()
    }
}

// BinaryHeap is a max-heap; highest priority and lowest sequence first.
impl Eq for QueuedWork {}

impl PartialEq for QueuedWork {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl PartialOrd for QueuedWork {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedWork {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.sequence.cmp(&self.sequence)) // lower sequence = earlier = higher priority
    }
}

/// Bounded, priority-ordered queue for inference work.
pub struct InferenceQueue {
    heap: BinaryHeap<QueuedWork>,
    capacity: usize,
    next_sequence: u64,
}

impl InferenceQueue {
    /// Create a new queue with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(capacity),
            capacity,
            next_sequence: 0,
        }
    }

    /// Enqueue a work item. Returns error if queue is full.
    pub fn enqueue(
        &mut self,
        priority: Priority,
        request_id: u64,
        kind: WorkKind,
    ) -> Result<(), QueueError> {
        if self.heap.len() >= self.capacity {
            return Err(QueueError::Full(self.capacity));
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.heap.push(QueuedWork {
            priority,
            request_id,
            kind,
            sequence,
        });
        Ok(())
    }

    /// Dequeue the highest-priority work item.
    pub fn dequeue(&mut self) -> Option<QueuedWork> {
        self.heap.pop()
    }

    /// Number of items currently in the queue.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_enqueue_dequeue_roundtrip() {
        let mut queue = InferenceQueue::new(10);
        let (tx, _rx) = oneshot::channel();
        queue
            .enqueue(
                Priority::Normal,
                1,
                WorkKind::Infer {
                    prompt: "hello".to_string(),
                    response_tx: tx,
                },
            )
            .expect("enqueue should succeed");

        assert_eq!(queue.len(), 1);

        let work = queue.dequeue().expect("dequeue should return item");
        assert_eq!(work.request_id, 1);
        assert_eq!(work.priority, Priority::Normal);
    }

    #[test]
    fn queue_priority_ordering() {
        let mut queue = InferenceQueue::new(10);

        let (tx1, _) = oneshot::channel();
        let (tx2, _) = oneshot::channel();
        let (tx3, _) = oneshot::channel();

        queue
            .enqueue(
                Priority::Low,
                1,
                WorkKind::Infer {
                    prompt: "low".to_string(),
                    response_tx: tx1,
                },
            )
            .expect("enqueue");
        queue
            .enqueue(
                Priority::High,
                2,
                WorkKind::Infer {
                    prompt: "high".to_string(),
                    response_tx: tx2,
                },
            )
            .expect("enqueue");
        queue
            .enqueue(
                Priority::Normal,
                3,
                WorkKind::Infer {
                    prompt: "normal".to_string(),
                    response_tx: tx3,
                },
            )
            .expect("enqueue");

        let first = queue.dequeue().expect("dequeue");
        assert_eq!(first.request_id, 2); // High
        let second = queue.dequeue().expect("dequeue");
        assert_eq!(second.request_id, 3); // Normal
        let third = queue.dequeue().expect("dequeue");
        assert_eq!(third.request_id, 1); // Low
    }

    #[test]
    fn queue_fifo_within_same_priority() {
        let mut queue = InferenceQueue::new(10);

        let (tx1, _) = oneshot::channel();
        let (tx2, _) = oneshot::channel();
        let (tx3, _) = oneshot::channel();

        queue
            .enqueue(
                Priority::Normal,
                10,
                WorkKind::Embed {
                    text: "a".to_string(),
                    response_tx: tx1,
                },
            )
            .expect("enqueue");
        queue
            .enqueue(
                Priority::Normal,
                20,
                WorkKind::Embed {
                    text: "b".to_string(),
                    response_tx: tx2,
                },
            )
            .expect("enqueue");
        queue
            .enqueue(
                Priority::Normal,
                30,
                WorkKind::Embed {
                    text: "c".to_string(),
                    response_tx: tx3,
                },
            )
            .expect("enqueue");

        assert_eq!(queue.dequeue().expect("d").request_id, 10);
        assert_eq!(queue.dequeue().expect("d").request_id, 20);
        assert_eq!(queue.dequeue().expect("d").request_id, 30);
    }

    #[test]
    fn queue_bounded_capacity() {
        let mut queue = InferenceQueue::new(2);

        let (tx1, _) = oneshot::channel();
        let (tx2, _) = oneshot::channel();
        let (tx3, _) = oneshot::channel();

        queue
            .enqueue(
                Priority::Normal,
                1,
                WorkKind::Infer {
                    prompt: "a".to_string(),
                    response_tx: tx1,
                },
            )
            .expect("enqueue");
        queue
            .enqueue(
                Priority::Normal,
                2,
                WorkKind::Infer {
                    prompt: "b".to_string(),
                    response_tx: tx2,
                },
            )
            .expect("enqueue");

        let result = queue.enqueue(
            Priority::Normal,
            3,
            WorkKind::Infer {
                prompt: "c".to_string(),
                response_tx: tx3,
            },
        );
        assert!(result.is_err());
        match result {
            Err(QueueError::Full(cap)) => assert_eq!(cap, 2),
            _ => panic!("Expected QueueError::Full"),
        }
    }

    #[test]
    fn queue_empty_dequeue_returns_none() {
        let mut queue = InferenceQueue::new(10);
        assert!(queue.dequeue().is_none());
        assert!(queue.is_empty());
    }
}
