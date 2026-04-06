# Sena — Blocked Items Register
**Status:** Living document — update when items are unblocked or new items discovered  
**Last updated:** Post-M6  
**Purpose:** Track items that cannot be implemented in the current phase due to an unresolved architectural decision, a dependency gap, or an explicit deferral to a future phase.

Each item has:
- An **ID** (prefix indicates category: R = runtime, B = bus, I = inference, SP = speech, Q = product question)
- A **blocking reason** (why it cannot be done now)
- A **resolution path** (what must happen to unblock it)
- A **target phase** (when it is expected to be resolved)

---

## R1 — Actor Restart Factory Pattern

**Status:** BLOCKED — design decision required  
**Crates affected:** `runtime`  
**Phase target:** Phase 8 (resilience hardening)

### Problem

`crates/runtime/src/registry.rs` says "Liveness monitoring (restart policy) is a stub for Phase 1." As of Phase 6, no restart mechanism exists. If an actor panics (e.g., `InferenceActor` crashes mid-generation, `SoulActor` hits a redb write fault), the actor disappears silently. The `ActorRegistry` detects the panic via `JoinHandle` poll, but takes no corrective action — it logs a join error and moves on.

In a long-running background daemon (the primary usage model from Phase 6 onward), a crashed actor is an invisible failure. The user sees Sena stop responding without any indication of why.

### What needs to be decided

1. **Restart policy per actor type:**  
   - Stateful actors (Soul, Memory) must NOT restart blindly — a restart without replaying state may corrupt the event log or memory graph.  
   - Stateless workers (Inference, Platform, CTP) are more safely restartable.  
   - Should the policy be: restart stateless actors up to N times, then emit `ActorFailed` and degrade gracefully?

2. **Factory abstraction:** A restart requires re-constructing the actor with the same configuration as at boot. This needs an `ActorFactory` trait or a closure-based strategy stored in the registry at boot time. The current registry only holds `JoinHandle<()>` — it has no constructor.

3. **Bus notification on crash:** When an actor fails and is restarted, dependent actors and the CLI need to know (e.g., "Inference restarting — pausing CTP..."). This requires new `SystemEvent` variants: `ActorFailed { name }` and `ActorRestarted { name }`.

### Resolution path

1. Decide restart policy matrix (which actors get N retries vs graceful degrade).
2. Add `ActorFactory` closure to `ActorRegistry::register()`.
3. Add `SystemEvent::ActorFailed` / `ActorRestarted` to `bus/events/system.rs`.
4. Implement restart loop in supervisor or registry's liveness monitor.
5. Update CLI to render `ActorFailed` / `ActorRestarted` as informational sidebar messages.

---

## B2 — Guaranteed Delivery for Critical Bus Events

**Status:** BLOCKED — architectural decision required  
**Crates affected:** `bus`, `runtime`  
**Phase target:** Phase 8 (resilience hardening)

### Problem

`EventBus` uses `tokio::sync::broadcast` channels. Broadcast channels are lossy by design: if a receiver's internal ring buffer fills up (i.e., the receiver is processing slowly), older messages are dropped and the receiver gets a `RecvError::Lagged` error. The current bus does not distinguish between "informational" events (safe to drop) and "critical" events (must not be dropped).

Examples of critical events where loss is problematic:
- `SystemEvent::ShutdownSignal` — if a slow actor misses this, it does not stop cleanly
- `SystemEvent::ConfigReloaded` — if missed, an actor runs with stale config until next restart
- `SoulEvent::EventLogged` — if missed by the IPC server, the user may not see a transparency log entry
- `InferenceEvent::InferenceFailed` — must reach the CLI; if dropped, the user sees no error

The current ring buffer capacity is set per-channel. Under load or on slow channels, important events can be silently dropped.

### What needs to be decided

1. **Tiered delivery model:** Should the bus support both a lossy broadcast tier (current) and a guaranteed-delivery directed channel tier (like the existing inference/memory mpsc side-channels)?  
   - Pattern: critical events use per-subscriber `mpsc` channels with backpressure. Informational events (heartbeat, telemetry) stay lossy broadcast.  
   - This aligns with the architecture's existing mpsc side-channels for inference and memory, but requires the bus to know which events are critical.

2. **Alternatively: just increase buffer sizes** for the current broadcast channels, accept that under extreme load events can still be dropped, and rely on actor-level idempotency. This is simpler but not safe for shutdown sequencing.

3. **Shutdown sequencing specifically:** The supervisor already uses a dedicated shutdown coordination path. This may be sufficient for the shutdown use case if other critical events are handled separately.

### Resolution path

1. Audit all event types for delivery sensitivity (must-deliver vs can-drop).
2. Decide: tiered bus model vs buffer tuning vs per-event mpsc subscription API.
3. If tiered: add `bus.subscribe_critical(event_type)` API returning a `mpsc::Receiver`.
4. Migrate critical event subscriptions to the guaranteed channel.
5. Document delivery contract per event type in `bus/src/events/`.

---

## I4 — Vision Frames Cannot Safely Route via Bus Events

**Status:** BLOCKED — architectural workaround in use; formal resolution deferred  
**Crates affected:** `inference`, `platform`, `runtime`  
**Phase target:** Phase 9 (multimodal)

### Problem

Screen capture produces raw image frames (PNG bytes, typically 200KB–5MB depending on resolution). Routing these on the broadcast bus as event payload would saturate the channel and increase memory pressure for every subscriber that does not need the vision data (Soul, Memory, CTP, Speech all subscribe to broadcast — they would each get a clone of 5MB frames at every capture interval).

The current workaround is a shared `Arc<Mutex<Option<Vec<u8>>>>` (the "vision frame store") passed directly between `PlatformActor` and `InferenceActor` at boot time via `InferenceActor::with_vision_frame_store()`. This bypasses the bus entirely for the actual frame data.

This violates the actor isolation principle: `copilot-instructions.md §4.5` says actors communicate via bus events, not direct function calls or shared state. The vision frame store is shared mutable state bridging two actors — exactly what the architecture prohibits.

### Why it is tolerated now

The alternative (bus events with large payloads) is worse than the workaround. For single-client Phase 6, the workaround is safe in practice. It becomes unsafe in Phase 9+ if multiple consumers (e.g., a future face-recognition actor or a screen-analysis actor) need concurrent read access to the frame without going through `InferenceActor`.

### Resolution path

1. Define a dedicated `ScreenCapturChannel` — a single-producer, multi-consumer channel separate from the EventBus. This is not a bus event channel; it is a typed streaming channel for large binary payloads.
2. `PlatformActor` writes frames to this channel. Any consumer (Inference, future vision actors) subscribes independently.
3. The bus carries only metadata events: `PlatformEvent::ScreenCaptureTaken { frame_id, timestamp, width, height }`.
4. Consumers request the frame by `frame_id` from the `ScreenCaptureChannel` — frames are reference-counted and dropped when all subscribers release them.
5. This pattern extends cleanly to audio streams (see B2 above, and SP1 in `SUBSYSTEM_AUDIT.md`).

### Prerequisite decisions

- What is the maximum frame retention window? (Current: `vision_frame_max_age_secs` config)
- Should the channel be bounded (drop oldest frame when buffer full) or unbounded (accumulate until OOM)?

---

## SP2 — Audio Device Hot-Swap

**Status:** DEFERRED — Phase 7  
**Crates affected:** `speech`, `platform`  
**Phase target:** Phase 7 (M7.1 or as part of audio mux refactor)

### Problem

When the user changes the active audio input device via `/microphone select <index>`, the config is saved but the running `AudioInputStream` in `SttActor` and `WakewordActor` are not updated. The change takes effect only on the next Sena restart.

This means:
- `/microphone` gives no feedback that a restart is needed, except a saved-config confirmation.
- The CLI says "Saved. Restart Sena to use the new model" — but this is not surfaced as a hard requirement to the user.
- In daemon mode, the user has no restart affordance from the CLI (there is no `/restart` command).

### Dependency

This is blocked on the audio stream architecture decision from `SUBSYSTEM_AUDIT.md Finding 2` — the dual-stream problem. Hot-swap requires the ability to tear down and reconstruct an `AudioInputStream` cleanly, which in turn requires the `AudioCaptureManager` to be extracted from `SttActor` (Audit Finding 1).

Implementing hot-swap before the audio mux refactor risks creating a third copy of audio stream management logic.

### Resolution path

1. Complete `SUBSYSTEM_AUDIT.md Finding 1` (extract `AudioCaptureManager` from `SttActor`) at Phase 7 start.
2. Complete `SUBSYSTEM_AUDIT.md Finding 2` (role-handoff between wakeword and STT) at Phase 7 start.
3. Add `SystemEvent::AudioDeviceChanged { device_name: Option<String> }` to the bus.
4. `AudioCaptureManager` subscribes to this event and restarts its stream with the new device.
5. Both `SttActor` and `WakewordActor` pick up the new device on next capture cycle without restart.

---

## Q1 — System Sleep/Wake Awareness

**Status:** DEFERRED — Phase 7  
**Crates affected:** `platform`, `ctp`, `runtime`  
**Phase target:** Phase 7

### Problem

Sena currently has no awareness of system sleep/wake transitions. When the computer sleeps and wakes:
- The `PlatformActor` polling timer keeps firing (even if the system was suspended for hours), producing stale context signals.
- `CTPActor` may fire inference requests on context that is hours old (the "current window" from before sleep).
- `SttActor` audio capture stream may be in a broken state after wake (OS audio subsystem teardown/reinit during sleep).
- Memory consolidation timers may run on stale data.

On Windows, `WM_POWERBROADCAST`/`PBT_APMRESUMESUSPEND` messages signal wake. On macOS, `IORegisterForSystemPower` with `kIOMessageSystemHasPoweredOn`. On Linux, `logind` dbus signal or `/sys/class/power_supply/`.

### Resolution path

1. Add `PlatformAdapter::subscribe_power_events()` stub to the trait (all three OS implementations return `None` initially).
2. Add `PlatformEvent::SystemSleeping` / `PlatformEvent::SystemWaking` to `bus/events/platform.rs`.
3. Implement Windows variant in `crates/platform/src/windows.rs`.
4. `CTPActor` subscribes to `SystemSleeping` → pauses signal ingestion and trigger gate. Subscribes to `SystemWaking` → flushes signal buffer and forces an immediate fresh snapshot.
5. `SttActor` subscribes to `SystemSleeping` → gracefully pauses audio capture.
6. `MemoryActor` subscribes to `SystemSleeping` → defers any pending consolidation until wake.

---

## Q3 — Speaker Diarization

**Status:** DEFERRED — Phase 7 M7.6  
**Crates affected:** `speech`  
**Phase target:** Phase 7 (M7.6)

### Problem

When multiple people are speaking (e.g., a meeting, a call), Sena cannot distinguish who said what. All transcribed speech is attributed to a single anonymous speaker. This limits the usefulness of the `/listen` command in multi-party contexts and makes the memory record of verbal context less actionable.

### Dependency

Blocked on Phase 7 M7.5 (continuous streaming transcription) being stable first. Diarization requires segmented audio chunks that align with speaker boundaries — which requires the streaming transcription pipeline to produce accurately timed segments.

Also blocked on model research: the choice between `pyannote.audio` ONNX export, Silero VAD + speaker embeddings, or a custom lightweight model must be made and documented in `docs/speech/speaker-diarization-model-choice.md` before implementation begins (ROADMAP OQ item at M7.6).

### Resolution path

Tracked in ROADMAP.md M7.6. No additional design decisions needed here beyond the model choice. Entry gate: M7.5 stable.

---

## Q5 — LLM Output Safety Filter

**Status:** BLOCKED — product decision required  
**Crates affected:** `inference`, `prompt`, `cli`  
**Phase target:** Unscheduled (product decision gating)

### Problem

Sena is a local-first, privacy-preserving AI assistant. Its LLM inference runs entirely on-device with no cloud moderation layer. The current pipeline produces raw model output and displays it directly to the user without any safety or quality filtering.

For the personal assistant use case (single user, their own device, their own data), this is acceptable — the user is the owner and the only consumer of model outputs. However, as Sena evolves, two questions must be answered:

**Q5a — Is a safety filter needed at all?**  
Arguments against: local-first means the user controls everything; adding a filter adds latency and complexity; the user chose the model and accepts its output characteristics.  
Arguments for: even personal-use assistants benefit from output quality signals (e.g., "model seems confused — low confidence output" detection); a toxic-output guard protects against jailbroken/corrupted model weights.

**Q5b — If filtering is needed, what layer owns it?**  
- `inference` crate: closest to the raw output; adds latency to every token; complicates streaming (token-level filter differs from sentence-level filter)
- `prompt` crate: post-processing hook after full response assembled; simpler but adds a round-trip before user sees final answer
- `cli` / display layer: rendering filter; never modifies the record, only what is shown; cleanest separation of concerns

**Q5c — Privacy constraint:**  
Any output filter that sends content to an external service violates Sena's P1 (local-first) guarantee. Filters must be purely local: rule-based heuristics, local classifier model, or keyword detection. No API calls.

### Resolution path

1. Developer decision required on Q5a first.
2. If yes: decide filter scope (safety only? quality signals? hallucination detection?) and expected latency budget.
3. Choose layer (Q5b) based on latency constraint.
4. Add `OutputFilter` trait to `crates/inference/` or `crates/prompt/` depending on layer decision.
5. Initial implementation: lightweight rule-based filter (repetition detection, confidence-based flagging) with a path to plug in a local classifier in Phase 8.

---

## Summary Table

| ID | Description | Category | Phase target | Blocker type |
|---|---|---|---|---|
| R1 | Actor restart factory pattern | Runtime resilience | Phase 8 | Design decision required |
| B2 | Guaranteed delivery for critical bus events | Bus architecture | Phase 8 | Architectural decision required |
| I4 | Vision frames cannot safely route via bus | Inference / Platform | Phase 9 | Architectural workaround in use |
| SP2 | Audio device hot-swap without restart | Speech | Phase 7 | Depends on audio mux refactor (Audit F1+F2) |
| Q1 | System sleep/wake awareness | Platform | Phase 7 | Platform API stubs needed |
| Q3 | Speaker diarization | Speech | Phase 7 M7.6 | Depends on M7.5 stable + model choice |
| Q5 | LLM output safety filter | Inference / Product | Unscheduled | Product decision required (Q5a) |
