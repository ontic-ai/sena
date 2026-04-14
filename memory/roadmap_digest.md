# Roadmap Digest
Source: docs/ROADMAP.md
Last synced: 2026-04-11

## Active Milestone: Phase 7 — Natural Speech: Voice Cloning and Continuous Listening

### Goal
Transform Sena's speech capabilities from basic TTS/STT to natural, emotionally-aware voice interaction. TTS gains voice cloning and emotional prosody driven by Soul state. STT becomes continuous, low-latency, and speaker-aware.

### Entry Gate
Phase 7A exit gate fully satisfied. OQ-TTS-7 and OQ-STT-7 resolved.

### Exit Gate
- [ ] All milestones M7.1–M7.6 checked off
- [ ] TTS uses StyleTTS2 with voice cloning on all 3 OS's
- [ ] Emotional prosody perceptibly different in calm vs urgent states (3/3 blind testers confirm)
- [ ] `/listen` command: < 2s latency for final results, unclear words flagged
- [ ] Speaker diarization accuracy > 85% in 2-speaker test
- [ ] Voice embedding encrypted at rest (hex-dump verified, no plaintext floats)
- [ ] StyleTTS2 OOM fallback to Piper works on < 8 GB VRAM configurations
- [ ] All Phase 1-6 exit gate conditions still hold

### Unchecked Items
**M7.1 — Soul Personality Schema Extension:**
- [ ] Schema migration v3: add `voice::urgency` [0,100] and `voice::stress_level` [0,100] identity signal keys
- [ ] Migration file: `crates/soul/src/schema/migrations/003_add_voice_emotion_signals.rs`
- [ ] Extend `PersonalityUpdated` event: add `urgency: u8`, `stress_level: u8` fields
- [ ] Soul actor `compute_personality()` reads new signals, defaults: urgency=30, stress=10
- [ ] Soul logs every `PersonalityUpdated` emission to event log
- [ ] Unit tests: migration v3 clean, defaults correct, event logged

**M7.2 — StyleTTS2 Backend Integration (TTS Layer 1):**
Requires OQ-TTS-7 resolved.
- [ ] Add ONNX runtime dependency
- [ ] New `TtsBackend::StyleTTS2` variant
- [ ] `crates/speech/src/styletts2_backend.rs`: ONNX model loading + synthesis
- [ ] TTS actor integration: route to StyleTTS2 when active
- [ ] Add StyleTTS2 ONNX model URLs to download manifest
- [ ] Config: `tts_backend_preference` field
- [ ] Integration tests: synthesis non-empty on all 3 OS's
- [ ] Exit: synthesis latency < 500ms/sentence on target hardware

**M7.3 — Voice Cloning (TTS Layer 3):**
Requires M7.2 complete, OQ-VOICE-7 resolved.
- [ ] New slash command `/voice clone`
- [ ] STT actor captures 10s audio → StyleTTS2 speaker encoder → 256-dim embedding
- [ ] Soul actor stores encrypted embedding in `VOICE_EMBEDDINGS` redb table
- [ ] TTS actor reads embedding from Soul at boot
- [ ] `/voice reset` slash command
- [ ] New bus events: `VoiceCloneRequested`, `VoiceCloneCompleted`, etc.
- [ ] Privacy: embedding never logged, encrypted at rest

**M7.4 — Emotional Prosody (TTS Layer 4):**
Requires M7.1 and M7.3 complete.
- [ ] StyleTTS2 backend: `compute_style_embedding(warmth, urgency, stress)` 
- [ ] TTS actor subscribes to `PersonalityUpdated`
- [ ] CTP identity signal path: cadence → urgency/stress
- [ ] Integration test: high urgency → urgent style

**M7.5 — `/listen` CLI Command (STT Layer 1):**
PARTIALLY IMPLEMENTED — remaining items:
- [x] New slash command `/listen` in `crates/cli/src/shell.rs`
- [x] CLI dispatches `SpeechEvent::ListenModeRequested`
- [x] STT actor: continuous capture, silence-triggered transcription
- [x] Listen mode uses independent `SilenceDetector`
- [ ] CLI renders: partial results in gray (overwritten), final results in white
- [x] Ctrl+C → `ListenModeStopRequested` → clean exit
- [ ] `[unclear]` in red for confidence < 0.6
- [x] New bus events: defined and wired
- [ ] Integration test: mock STT → CLI renders correctly
- [ ] Exit: partial results < 1s, final results < 2s from silence

**M7.6 — Speaker Diarization (STT Layer 2):**
Requires M7.5 complete.
- [ ] Research phase: model choice documented
- [ ] `crates/speech/src/diarization.rs`: speaker embeddings
- [ ] Extend `ListenModeTranscription`: add `speaker_id`
- [ ] CLI displays speaker labels
- [ ] Integration test
- [ ] Exit: > 85% accuracy in 2-speaker test

### Open Questions (Phase 7)
| # | Question | Blocks |
|---|---|---|
| OQ-TTS-7 | StyleTTS2 integration path: ONNX + onnxruntime-rs vs Python FFI via pyo3? ONNX preferred if latency < 500ms and cross-platform. | M7.2 |
| OQ-STT-7 | Does whisper-rs support streaming incremental transcription? Define max chunk size for < 800ms latency. | M7.4 |
| OQ-VOICE-7 | Voice cloning embeddings are biometric data. Schema: new Soul table VOICE_EMBEDDINGS AES-256-GCM. Deletion policy. | M7.3 |

## Next Milestone: Planned Features — Assistant Evolution Backlog

## Completed Milestones
- Phase 1 — Foundation: The Bus and Boot (M1.1–M1.8) ✓
- Phase 2 — Inference and Persistence (M2.0–M2.7) ✓
- Phase 3 — Intelligence: CTP and Soul Growth (M3.1–M3.6) ✓
- Phase 4 — Surface and Polish (M4.1–M4.4) ✓
- Phase 5 — Speech: Primary Interaction Surface (M5.1–M5.6) ✓
- M-Refactor — Runtime as Process Owner ✓
- Phase 5.5 — Streaming Inference and Ordered TTS (M5.5.1–M5.5.5) ✓
- Phase 6 — CLI Decoupling and Configuration (M6.1–M6.4) ✓
- Phase 7A — CTP + Soul Intelligence Layer & UX Polish (M7A.1–M7A.7) ✓
