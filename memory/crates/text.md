# Crate: text
Path: crates/text/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
Sentence boundary detection and text utilities. A pure-function leaf node crate with zero dependencies (not even external crates). Used by inference for streaming sentence extraction.

## Public API Surface
**Functions:**
- `detect_sentence_boundary(buffer: &str, max_buffer_chars: usize, max_sentence_chars: usize) -> Option<(String, String)>`

**Module:**
- `sentence` — sentence boundary detection logic

## Bus Events Owned
None — text is a utility crate

## Dependency Edges
Imports from Sena crates: (none — leaf node)
Imported by Sena crates: inference, prompt
Key external deps: (none)

## Background Loops Owned
None

## Known Issues
None

## Notes
- Pure function: no state, no side effects, deterministic
- Boundary rules (priority order):
  1. Hard boundary: `.`, `?`, `!` followed by whitespace/end
  2. Soft boundary: `;` followed by whitespace
  3. Comma threshold: `,` when buffer > max_buffer_chars
  4. Hard cap: split at whitespace when buffer > max_sentence_chars
- 22+ unit tests covering all boundary conditions
- Config thresholds: `inference.streaming.max_buffer_chars` (150), `inference.streaming.max_sentence_chars` (400)
