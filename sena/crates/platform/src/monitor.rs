//! Vision frame cache — thread-safe ring buffer for recent screen captures.

use std::collections::VecDeque;
use std::sync::Mutex;

/// Maximum number of frames to retain.
const MAX_FRAMES: usize = 10;

/// Thread-safe cache of recent screen frames (raw RGB bytes).
pub struct VisionFrameCache {
    frames: Mutex<VecDeque<Vec<u8>>>,
}

impl VisionFrameCache {
    /// Create an empty frame cache.
    pub fn new() -> Self {
        Self {
            frames: Mutex::new(VecDeque::with_capacity(MAX_FRAMES)),
        }
    }

    /// Add a new frame, evicting the oldest if at capacity.
    pub fn add_frame(&self, rgb_data: Vec<u8>) {
        let mut frames = self.frames.lock().expect("VisionFrameCache lock poisoned");
        if frames.len() >= MAX_FRAMES {
            frames.pop_front();
        }
        frames.push_back(rgb_data);
    }

    /// Return the most recent frame, if any.
    pub fn latest_frame(&self) -> Option<Vec<u8>> {
        let frames = self.frames.lock().expect("VisionFrameCache lock poisoned");
        frames.back().cloned()
    }

    /// Number of frames currently cached.
    pub fn len(&self) -> usize {
        let frames = self.frames.lock().expect("VisionFrameCache lock poisoned");
        frames.len()
    }

    /// True when no frames are cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for VisionFrameCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_retrieve_frame() {
        let cache = VisionFrameCache::new();
        assert!(cache.is_empty());
        cache.add_frame(vec![1, 2, 3]);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.latest_frame(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn evicts_oldest_when_full() {
        let cache = VisionFrameCache::new();
        for i in 0..=MAX_FRAMES {
            cache.add_frame(vec![i as u8]);
        }
        assert_eq!(cache.len(), MAX_FRAMES);
    }
}
