//! Text processing utilities for Sena.
//!
//! This crate provides text boundary detection and manipulation.
//! Currently includes sentence boundary detection types.

mod sentence;

pub use sentence::{
    SentenceBoundary, SentenceBoundaryIterator, SentenceSplitter, detect_sentence_boundary,
};
