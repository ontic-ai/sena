//! Working memory — ephemeral, in-RAM short-term context.
//!
//! Working memory holds recent observations during a session and is never persisted.
//! It is flushed to long-term memory (ech0) during the consolidation pass.

use std::time::Instant;

/// A single chunk of working memory.
#[derive(Debug, Clone)]
pub struct WorkingMemoryChunk {
    /// Text content of this chunk.
    pub text: String,
    /// When this chunk was added to working memory.
    pub timestamp: Instant,
}

impl WorkingMemoryChunk {
    /// Create a new working memory chunk with the current timestamp.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            timestamp: Instant::now(),
        }
    }
}

/// Ring-buffer of recent working memory chunks.
///
/// When the buffer reaches `max_size`, the oldest chunk is evicted on the next push.
pub struct WorkingMemory {
    chunks: Vec<WorkingMemoryChunk>,
    max_size: usize,
}

impl WorkingMemory {
    /// Create a new working memory buffer with the given capacity.
    pub fn new(max_size: usize) -> Self {
        Self {
            chunks: Vec::with_capacity(max_size),
            max_size,
        }
    }

    /// Push a new chunk into working memory.
    ///
    /// If the buffer is at capacity, the oldest chunk is evicted first.
    pub fn push(&mut self, chunk: WorkingMemoryChunk) {
        if self.chunks.len() >= self.max_size {
            self.chunks.remove(0);
        }
        self.chunks.push(chunk);
    }

    /// Read all chunks currently in working memory.
    pub fn chunks(&self) -> &[WorkingMemoryChunk] {
        &self.chunks
    }

    /// Clear all chunks from working memory.
    pub fn clear(&mut self) {
        self.chunks.clear();
    }

    /// Number of chunks currently in working memory.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Returns `true` if working memory is empty.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_stores_chunk() {
        let mut wm = WorkingMemory::new(10);
        wm.push(WorkingMemoryChunk::new("hello"));
        assert_eq!(wm.len(), 1);
        assert_eq!(wm.chunks()[0].text, "hello");
    }

    #[test]
    fn push_evicts_oldest_when_at_capacity() {
        let mut wm = WorkingMemory::new(2);
        wm.push(WorkingMemoryChunk::new("first"));
        wm.push(WorkingMemoryChunk::new("second"));
        wm.push(WorkingMemoryChunk::new("third"));
        assert_eq!(wm.len(), 2);
        assert_eq!(wm.chunks()[0].text, "second");
        assert_eq!(wm.chunks()[1].text, "third");
    }

    #[test]
    fn clear_empties_buffer() {
        let mut wm = WorkingMemory::new(10);
        wm.push(WorkingMemoryChunk::new("a"));
        wm.clear();
        assert!(wm.is_empty());
    }
}
