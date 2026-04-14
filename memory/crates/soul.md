# Crate: soul
Path: crates/soul/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
SoulBox: identity schema, event log, and personalization state. Sena's identity engine built on redb. Soul accumulates over time and never resets — deletion is a deliberate user action. Includes intelligence layer for pattern distillation, temporal modeling, and preference learning.

## Public API Surface
**Types:**
- `SoulActor` — the soul orchestrator actor
- `EncryptedDb` — encrypted redb wrapper
- `SoulError` — error enum
- `Redacted` — log-safe wrapper
- `DistillationEngine` — identity pattern extraction
- `PreferenceLearner` — engagement signal tracking
- `SummaryAssembler` — RichSoulSummary builder
- `TemporalModel` — hour/day activity buckets

**Functions:**
- `apply_schema()` — run migrations

**Modules:**
- `actor` — SoulActor implementation
- `encrypted_db` — encrypted redb
- `error` — error types
- `redacted` — safe logging
- `schema` — tables and migrations
- `distillation` (private) — identity signal harvesting
- `preference_learning` (private) — engagement tracking
- `summary_assembler` (private) — rich summary generation
- `temporal_model` (private) — time-based patterns

## Bus Events Owned
Emits (defined in bus):
- `SoulEvent::SoulSummaryReady`
- `SoulEvent::SoulEventLogged`
- `SoulEvent::IdentitySignalDistilled`
- `SoulEvent::TemporalPatternDetected`
- `SoulEvent::PreferenceLearningUpdate`
- `SoulEvent::RichSummaryReady`

Subscribes to:
- `SoulEvent::SoulSummaryRequested`
- `SoulEvent::RichSummaryRequested`
- `InferenceEvent::InferenceCompleted` — logs cycles
- `MemoryEvent::*` — logs memory operations
- (various) — absorbs events for identity signals

## Dependency Edges
Imports from Sena crates: bus, crypto
Imported by Sena crates: runtime, memory
Key external deps:
- redb (v3.1.2) — embedded database
- tokio
- tracing

## Background Loops Owned
None — event-driven

## Known Issues
- TODO: M6 — implement full export (event log + identity signals → JSON)

## Notes
**Soul Intelligence Layer (Phase 7A):**

1. **Distillation Engine:**
   - Watches identity signal counters
   - Distills patterns when threshold crossed (>5 occurrences in 7 days)
   - Examples: preferred languages, project contexts, communication style

2. **Temporal Model:**
   - Buckets events by hour (0–23) and day (Mon–Sun)
   - Peak activity hours, work vs off-hours, weekend vs weekday

3. **Preference Learner:**
   - Tracks InferenceAccepted/Ignored/Interrupted, FollowUpQuery
   - After 20+ feedback events: distills verbosity, engagement, proactiveness

4. **Summary Assembler:**
   - Produces RichSoulSummary: RecentEvents, IdentitySignals, TemporalHabits, Preferences
   - Relevance-scored sections for token budget allocation

**Harvest cycle:** every 50 absorbed events

**Hard rules:**
- Soul starts empty — never pre-seeded
- Schema changes require migrations
- No other crate writes to Soul redb directly (mpsc channel only)
- Event log is append-only (no system deletion)
- Soul's internal types never exposed in pub APIs
- Soul does not perform inference
- All intelligence modules are `mod` (private)
