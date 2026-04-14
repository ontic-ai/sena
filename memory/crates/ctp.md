# Crate: ctp
Path: crates/ctp/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
Continuous Thought Processing — Sena's observation loop. Sits between platform signals and higher-level reasoning. Implements the pipeline: Platform Events → Signal Buffer → Context Assembler → Trigger Gate → ThoughtEvent. This is Sena's core differentiator.

## Public API Surface
**Types:**
- `CTPActor` — the CTP orchestrator actor

**Modules:**
- `context_assembler` — transforms buffer to ContextSnapshot
- `ctp_actor` — main actor implementation
- `pattern_engine` — behavioral pattern detection
- `signal_buffer` — rolling time-window accumulator
- `task_inference` — semantic task descriptions
- `transparency_query` — /observation command handler
- `trigger_gate` — decides when to emit ThoughtEvent
- `user_state` — cognitive state classification

## Bus Events Owned
Emits (defined in bus):
- `CTPEvent::ContextSnapshotReady`
- `CTPEvent::ThoughtEventTriggered`
- `CTPEvent::UserStateComputed`
- `CTPEvent::SignalPatternDetected`
- `CTPEvent::EnrichedTaskInferred`

Subscribes to:
- `PlatformEvent::*` — all platform signals
- `MemoryEvent::ContextMemoryQueryResponse` — memory relevance feedback

## Dependency Edges
Imports from Sena crates: bus, platform
Imported by Sena crates: runtime
Key external deps:
- tokio (async)
- async-trait
- thiserror
- tracing

## Background Loops Owned
- `ctp` — continuous thought processing loop

## Known Issues
None in production paths

## Notes
**CTP Pipeline:**
1. Signal Buffer — rolling N-second window of platform events
2. Context Assembler — transforms to typed ContextSnapshot
3. Trigger Gate — significance-based decision

**Pattern Engine (Phase 7A):**
- Frustration: rapid window switches, keystroke variance, abandoned clipboard
- Repetition: same file edited repeatedly, app back-and-forth
- FlowState: sustained cadence, no switches, low idle
- Anomaly: out-of-hours, unusual app combo

**User State Classifier:**
- frustration_level: 0–100
- flow_detected: bool
- context_switch_cost: 0–100
- Ephemeral — computed per tick, never persisted

**Task Inference:**
- Generates semantic descriptions from window context
- Rule-based, no LLM
- Must be <5ms (synchronous)

**Trigger Scoring:**
- Context diff: 40%
- Pattern detection: 30%
- Memory relevance: 20%
- User state: 10%

**Signal Completeness Rule:**
If Sena observes it, CTP must know about it.
