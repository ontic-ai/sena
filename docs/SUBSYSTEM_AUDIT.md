# Sena — Subsystem Responsibility Audit
**Status:** PLAN ONLY — do not execute without developer approval  
**Trigger:** Developer question: "Shouldn't continuous listening be part of CTP?"  
**Scope:** Full cross-crate responsibility boundary analysis  
**Date:** Current session (post-M6.3/M6.4 completion)

---

## How to Use This Document

Each finding below has a severity:

| Severity | Meaning |
|---|---|
| **CRITICAL** | Violates a hard architecture rule; will cause bugs or build failures |
| **HIGH** | Clear responsibility bleed that will compound as phases progress |
| **MEDIUM** | Design debt that is tolerable now but will hurt in Phase 7+ |
| **LOW** | Cosmetic or minor; address when nearby code is touched |

Each finding has a proposed resolution and a cost estimate. Nothing in this document is executed until you say so.

---

## Finding 1 — STT Actor owns multiple independent responsibilities

**Severity: HIGH**

**Files:** `crates/speech/src/stt_actor.rs`

**Observed:** The single `SttActor` struct currently owns:
1. Backend initialization (Whisper model loading, CPU-bound, heavy)
2. Always-on background audio capture (`voice_always_listening` mode)
3. Wakeword-triggered capture (response to `WakewordDetected` events)
4. Explicit user-initiated listen sessions (`/listen` command, `ListenModeRequested`)
5. Voice activity detection and silence-threshold logic
6. Actual transcription (delegating to Whisper worker)
7. Audio device selection

Architecture §14.1 says speech is "two independent actors: STT Actor and TTS Actor." The STT actor was written as one actor, but it is de-facto three concerns:
- A capture manager (microphone ownership, device selection, start/stop lifecycle)
- A VAD/silence detector (two copies of the same logic: `handle_audio_buffer()` and `handle_listen_audio_buffer()`)
- A transcription engine (model I/O)

**Why it matters:** Phase 7 adds continuous streaming transcription (M7.5 is already done early), voice cloning, and speaker diarization. Each of these requires mutating the capture and transcription layers independently. With everything in one actor, a change to the silence detection logic risks breaking the always-on capture path and vice versa.

**Proposed resolution:**
- Extract a `AudioCaptureManager` struct (not actor) inside `crates/speech/` that owns the `AudioInputStream` lifecycle and device resolution. Both STT and WakewordActor can construct their own `AudioCaptureManager` with isolated state.
- Deduplicate the silence detection logic into a single `SilenceDetector` struct — the two copies (`handle_audio_buffer` and `handle_listen_audio_buffer`) are 90% identical.
- Keep `SttActor` as the public actor, but have it delegate to these two internal structs. This is a refactor entirely within `crates/speech/` with no API surface changes.

**Cost:** Medium. Internal-to-speech refactor. No cross-crate changes.

---

## Finding 2 — Wakeword and STT both hold simultaneous live audio streams from the same device

**Severity: HIGH**

**Files:** `crates/speech/src/wakeword.rs`, `crates/speech/src/stt_actor.rs`

**Observed:** When `voice_always_listening = true`:
- `WakewordActor::start()` opens a `cpal` stream via `AudioInputStream::start(config)` with `device_name: None`
- `SttActor::maybe_start_audio_capture()` opens a second `cpal` stream via `AudioInputStream::start(config)` with `device_name: self.microphone_device.clone()`

Boot order (architecture §4.1 step 11) starts both actors without coordination. On Windows/WASAPI, two exclusive streams to the same device may fail silently or produce dropped packets. On macOS CoreAudio and Linux ALSA, shared access is allowed, but each stream receives the same audio independently, doubling CPU processing for no benefit.

The WakewordActor detects the wakeword and emits `WakewordDetected`. The STT actor receives that event and starts capturing again — potentially opening a third stream.

**Why it matters:** This is a resource management bug disguised as a design issue. It will manifest as higher-than-expected CPU usage, intermittent capture failures on Windows, and confusing behavior when a non-default device is selected via `/microphone` (wakeword still uses system default, STT uses device name — they capture from different devices).

**Proposed resolution:** The audio input layer should be shared and arbitrated. Two options:

*Option A — Single audio broadcast channel:* Create one `AudioInputStream` owned by a new thin `AudioMuxActor` (or owned by `WakewordActor` as the "primary" capture owner). This actor broadcasts raw `AudioBuffer` events on the bus (or via a specialized mpsc fan-out). `WakewordActor` and `SttActor` subscribe to the shared stream instead of opening their own. Requires a new bus channel type for audio data.

*Option B — Role handoff:* When STT activates in always-listening mode, WakewordActor pauses (stops its stream) and STT takes over. When STT deactivates, WakewordActor resumes. This requires a `WakewordPauseRequested`/`WakewordResumeRequested` event pair. The suppression flag already in `WakewordActor` is a partial implementation of this idea.

Option B is lower-cost and compatible with the current architecture. Option A is architecturally cleaner for Phase 7 (diarization needs a single source of truth for audio).

**Cost:** Medium-High. Requires new bus events and changes to both speech actors.

---

## Finding 3 — Continuous listening governance: should CTP decide when to activate STT?

**Severity: MEDIUM**

**Files:** `crates/ctp/src/ctp_actor.rs`, `crates/speech/src/stt_actor.rs`

**Observed:** The user's question was the trigger for this audit. The current state is:
- STT activates based on: user config flag (`voice_always_listening`), user command (`/listen`), or wakeword detection
- CTP never emits an event that activates or deactivates STT
- STT transcriptions (`TranscriptionCompleted`) are received by CLI for display but are **not fed back into CTP's `SignalBuffer`**

This means audio speech is not a contextual signal in Sena's observation loop. CTP knows about: active window, clipboard, files, keystroke cadence, and screen captures. It has no awareness of what the user is saying out loud.

**Two distinct sub-questions:**

**3a — Should speech transcriptions be a CTP signal?**  
Yes, architecturally this makes sense. Speech is a sensing modality equivalent to clipboard — it is ambient user activity. If Sena hears the user say "I need to finish the report by Friday," that should influence `ContextSnapshot` just as a file event would. The fix: CTP subscribes to `SpeechEvent::TranscriptionCompleted` and pushes a `SpeechDigest` signal into `SignalBuffer`. Architecture allows this (CTP → bus, speech → bus, no circular dependency). Requires a new `SignalBuffer::push_speech()` and a new `ContextSnapshot::speech_digest` field.

**3b — Should CTP decide to activate continuous STT based on context?**  
This is more nuanced. The CTP trigger gate already decides when to fire inference. An analogous mechanism for STT activation would be: "when context indicates the user is in a verbal task (e.g., call, meeting, dictation context detected via window/app), auto-activate STT." This is a Phase 7 / Phase 8 capability. For current scope it is premature. However, if the architecture needs to support it in Phase 7, the event pathway must be planned now: `CTPEvent::SpeechCaptureRecommended` → STT actor subscribes → conditionally activates.

**Recommended resolution:**  
Sub-question 3a: Plan for M7.5+ — add `SpeechDigest` to `SignalBuffer` / `ContextSnapshot`. This is out-of-scope for current phases.  
Sub-question 3b: Do not implement until Phase 8 (backlog). Add a note to `ROADMAP.md` backlog section: "Context-triggered STT activation: CTP emits SpeechCaptureRecommended when meeting/call context detected."

**The answer to the user's question:**  
Continuous listening (as user-initiated) stays in `speech/stt_actor.rs`. The *governance* of when to suggest or auto-activate it belongs in CTP, but only for Phase 8+. The Phase 7 M7.5 `/listen` command is correct: it is user-initiated and belongs in speech.

**Cost:** Low (add ROADMAP comment). Medium if 3a is implemented.

---

## Finding 4 — CLI directly manages config instead of dispatching events

**Severity: HIGH**

**Files:** `crates/cli/src/shell.rs`

**Observed:** The `set_config_value()` function in `shell.rs` does:
```
load_or_create_config() → match key → modify field → save_config()
```
This pattern appears for every `/config set` invocation. The CLI:
- Reads the config file from disk directly
- Parses and mutates a `Config` struct
- Writes the config file back to disk

This is business logic in the CLI layer. It violates `architecture.md §4.3` ("CLI has no business logic") and `copilot-instructions.md §8.1` ("Every CLI command maps to exactly one bus event").

In Phase 6 (IPC), the CLI will be a separate process. A separate-process CLI cannot write directly to a config file that the daemon holds in memory — the daemon would be running with stale config while the file has changed. The `ConfigReloadRequested` broadcast partially mitigates this today, but it is a workaround for a architectural violation, not a fix.

Similarly, `/microphone select <index>` in `shell.rs` directly saves `microphone_device` to config. Same violation.

**Proposed resolution:** Define a `ConfigSetRequested { key: String, value: String }` event in `bus/events/system.rs`. CLI dispatches this event for every `/config set` invocation. The supervisor handles it: applies the change, saves the file, broadcasts `ConfigReloaded`. CLI renders the `ConfigReloaded` event as confirmation. Config file I/O is entirely in the daemon.

This also resolves the `ConfigReloadRequested` workaround: the daemon already owns the reload.

**Cost:** Medium. Requires new bus event, supervisor handler, and refactoring `set_config_value()` in `shell.rs`. No external API changes.

---

## Finding 5 — Token auto-tuner config writes in the supervision hot loop

**Severity: MEDIUM**

**Files:** `crates/runtime/src/supervisor.rs`, `crates/runtime/src/analytics.rs`

**Observed:** When `TokenTuner::record()` returns a recommendation, the supervisor:
1. Calls `load_or_create_config()` (disk read)
2. Mutates `inference_max_tokens`
3. Calls `save_config()` (disk write)
4. Broadcasts `TokenBudgetAutoTuned`

This disk I/O happens on the supervisor's async event loop without `spawn_blocking`. The `load_or_create_config` and `save_config` functions do synchronous file reads/writes. On a slow disk or under load this will hold up the supervision loop for every inference completion.

Additionally, re-reading the config from disk before writing risks overwriting any config changes that happened between the last read and this write (TOCTOU race for single-process writes, though low-probability today).

**Proposed resolution:**
- Supervisor should hold the current `Config` in memory (loaded at boot) and mutate it in place during auto-tune updates. Config writes should go through a `tokio::task::spawn_blocking` call.
- This is compatible with Finding 4's resolution: when `ConfigSetRequested` arrives, supervisor mutates its in-memory `Config` and spawns a blocking write task.

**Cost:** Low-Medium. Supervisor change only.

---

## Finding 6 — `analytics.rs` belongs in its own crate or in `ctp/`

**Severity: LOW**

**Files:** `crates/runtime/src/analytics.rs`

**Observed:** `TokenTuner` is currently in `crates/runtime/src/analytics.rs`. The `runtime` crate is the composition root — it boots actors, owns the supervision loop, and manages lifecycle. Analytics (observation of usage patterns and adaptive responses) is conceptually closer to what CTP does: observe, analyze, decide.

`TokenTuner` has zero dependencies on runtime internals — it is a pure data structure (`Vec`, `VecDeque`, arithmetic). It could live anywhere. Its placement in `runtime` causes `runtime` to grow beyond its intended role.

**Proposed resolution:** Either:
1. Move to `crates/ctp/src/analytics.rs` — CTP is the "observation and adaptation" crate; token budget is an inference parameter that CTP could tune based on context complexity
2. Create a new `crates/telemetry/` crate — but this adds crate overhead for one struct, which violates §1 workspace rules ("Never add a dependency to the workspace without confirming it is the correct, maintained crate")

Option 1 is preferred. CTP already has trigger gate sensitivity tuning; P95-based token tuning fits the same pattern.

However, this move would add a `ctp → inference` knowledge concern (CTP would be aware of inference token limits). Via the bus, this is fine — CTP emits `TokenBudgetRecommended` and the supervisor/inference actor applies it.

**Cost:** Low. Struct move + test move. No public API surface changes outside `runtime`.

---

## Finding 7 — `/listen` command implemented before its scheduled phase

**Severity: LOW (scope note, not a bug)**

**Files:** `crates/cli/src/shell.rs`, `crates/speech/src/stt_actor.rs`

**Observed:** The ROADMAP schedules `/listen` as M7.5 (Phase 7). It was implemented in Phase 5 (commit 739d412) with the listen mode events and continued in Phase 6 (commit e0d8e56 with `/microphone`). The implementation is complete and correct per the M7.5 specification.

This is not a bug. Early implementation of a planned feature is acceptable as long as the implementation matches the spec. The M7.5 checklist items should be reviewed:
- [x] `/listen` slash command in shell.rs
- [x] CLI dispatches `SpeechEvent::ListenModeRequested { session_id }`
- [x] STT actor continuous capture + `ListenModeTranscription { text, is_final, confidence, session_id }`
- [ ] Partial results in gray (currently all renders the same style) — **incomplete**
- [x] `is_final=true` after silence threshold
- [x] Ctrl+C → `ListenModeStopRequested` → `ListenModeStopped`
- [x] `[unclear]` for confidence < 0.6 — **partially** (low-confidence results are skipped, not labeled `[unclear]`)

**Proposed resolution:** Mark M7.5 as partially complete in `ROADMAP.md`. Note the two incomplete items. Do not back-port Phase 7 logic into Phase 6 scope.

**Cost:** Near-zero (ROADMAP edit only).

---

## Finding 8 — Platform crate does not have a stub for audio signals

**Severity: LOW**

**Files:** `crates/platform/`, `docs/architecture.md §5.2`

**Observed:** Architecture §5.2 lists four platform signal types: active window, clipboard, file events, keystroke patterns. Audio (microphone input) is a fifth platform signal, but it is handled exclusively inside `crates/speech/` using `cpal` directly — not through the `PlatformAdapter` trait.

This is architecturally inconsistent. Audio is an OS-level signal (device enumeration is OS-specific; WASAPI vs CoreAudio vs ALSA). By not routing it through `PlatformAdapter`, the platform-adaptation concern is scattered: window context is in `platform/`, but audio device context is in `speech/`.

**Why it matters for Phase 7:** Speaker diarization (M7.6) requires knowing which microphone is active and potentially switching devices. If audio device management bypasses `PlatformAdapter`, there is no single place where OS audio policy is defined.

**Proposed resolution (Phase 7+ only):** Add audio device discovery to `PlatformAdapter`:
```rust
fn list_audio_input_devices(&self) -> Vec<AudioDeviceInfo>;
fn default_audio_input_device(&self) -> Option<AudioDeviceInfo>;
```
The `list_input_devices()` function currently in `crates/speech/src/audio_input.rs` (and re-exported up through `runtime`) should long-term be delegated to `PlatformAdapter`. For current scope, the existing path is acceptable.

**Cost:** Low for planning note. Medium for implementation when Phase 7 begins.

---

## Summary Table

| # | Finding | Crates Affected | Severity | Proposed Action | Phase |
|---|---|---|---|---|---|
| 1 | STT actor owns too many concerns | `speech` | HIGH | Extract `AudioCaptureManager` + `SilenceDetector` structs internally | 7 |
| 2 | Dual simultaneous audio streams from same device | `speech` | HIGH | Role-handoff option B: wakeword pauses when STT activates | 7 |
| 3a | Speech transcriptions not in CTP signal buffer | `ctp`, `speech` | MEDIUM | Add `SpeechDigest` signal type to CTP | 7 |
| 3b | CTP does not govern STT activation | `ctp`, `speech` | MEDIUM | Backlog: Phase 8 feature, add ROADMAP note now | 8 |
| 4 | CLI writes config directly instead of dispatching events | `cli`, `runtime`, `bus` | HIGH | New `ConfigSetRequested` event; supervisor owns config I/O | 6 (before M6.2 IPC) |
| 5 | Config writes in supervision hot loop without spawn_blocking | `runtime` | MEDIUM | Supervisor holds in-memory config; writes via `spawn_blocking` | 6 |
| 6 | `analytics.rs` misplaced in `runtime` | `runtime`, `ctp` | LOW | Move `TokenTuner` to `ctp/analytics.rs` | 7 |
| 7 | `/listen` predates scheduled phase | `cli`, `speech` | LOW | Update ROADMAP.md M7.5 partial-complete status | Now |
| 8 | Audio device management outside `PlatformAdapter` | `platform`, `speech` | LOW | Plan `PlatformAdapter` audio API for Phase 7 start | 7 |

---

## Priority Order (for developer approval)

### Implement before M6.2 IPC (blockers if left):
1. **Finding 4** — CLI config writes: `ConfigSetRequested` event. If IPC is built before this is fixed, the IPC path will inherit the violation and be very hard to untangle.

### Implement at the start of Phase 7:
2. **Finding 2** — Dual audio streams: must be resolved before Phase 7 adds more audio consumers (diarization, voice cloning capture).
3. **Finding 1** — STT actor concerns: extract `AudioCaptureManager` and `SilenceDetector` before M7.3/M7.5 add more audio state.
4. **Finding 3a** — Speech into CTP signals: implement with M7.5 work (continuous transcription is the right time to feed transcriptions into the context snapshot).

### Can defer to when code is touched:
5. **Finding 5** — Async config writes (medium priority, fix alongside Finding 4).
6. **Finding 6** — Move `analytics.rs` (low friction, do when touching ctp or runtime).
7. **Finding 8** — `PlatformAdapter` audio API (plan at Phase 7 start, implement when CLI device selection is revisited).

### Document-only change:
8. **Finding 7** — ROADMAP M7.5 partial status (zero-cost, do now).

---

## Open Question Resolution

**"Shouldn't continuous listening be part of CTP?"**

The answer depends on what "continuous listening" means:

- **User-initiated continuous transcription** (`/listen` command) → stays in `speech`. This is a user action, not a context inference. ✓ correctly placed.  
- **Always-on ambient listening** (`voice_always_listening = true`) → stays in `speech`. The microphone is a sensor; CTP is the observer of sensors, not the owner of them. ✓ correctly placed.  
- **Feeding transcription results as context signals to CTP** → should flow to CTP (Finding 3a). Currently missing.  
- **CTP deciding to activate/deactivate STT based on context** → Phase 8 backlog. Premature to implement. Should be a ROADMAP note only.

The core principle: **sensors live in `platform` (or `speech` for audio). CTP observes sensor output via the bus. The decision of when a sensor is active is governance, not observation, and for Phase 7 that governance is user-controlled. CTP gets to see the results, but not yet to manage the lifecycle.**
