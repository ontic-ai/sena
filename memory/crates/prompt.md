# Crate: prompt
Path: crates/prompt/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
Dynamic prompt composition engine. Zero static prompt strings. All prompt content flows through typed PromptSegment variants. The composer is stateless — construct one per inference cycle.

## Public API Surface
**Types:**
- `PromptComposer` — assembles segments into prompt string
- `PromptSegment` — typed prompt fragment
- `ReflectionMode` — Iterative, SinglePass
- `PromptError` — composition errors

**Modules:**
- `composer` — PromptComposer implementation
- `error` — error types
- `segment` — PromptSegment enum and rendering

## Bus Events Owned
None — prompt is a utility crate, not an actor

## Dependency Edges
Imports from Sena crates: bus
Imported by Sena crates: (used directly by inference actor)
Key external deps:
- thiserror

## Background Loops Owned
None

## Known Issues
None in production paths

## Notes
**PromptSegment variants:**
- `SystemPersona(PersonaState)` — soul-driven persona
- `MemoryContext(Vec<MemoryChunk>)` — retrieved memories
- `CurrentContext(ContextSnapshot)` — CTP snapshot
- `UserIntent(Option<String>)` — user's explicit query
- `ReflectionDirective(ReflectionMode)` — multi-round control
- `SoulContext(SoulSummary)` — basic soul summary
- `RichSoulContext(RichSoulSummary)` — multi-section soul summary

**Phase 7A enhancements:**
- `RichSoulContext` renders: RecentEvents, IdentitySignals, TemporalHabits, Preferences
- `CurrentContext` includes: semantic task description + user state

**Hard rules:**
- No hardcoded strings in prompt assembly
- Token budget always respected via llama-cpp-rs tokenizer
- Pure transformation: inputs → prompt string (no side effects)
