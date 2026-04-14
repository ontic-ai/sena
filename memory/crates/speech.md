# Crate: speech
Path: crates/speech/
Last updated: 2026-04-12
Last commit touching this crate: b29c124 — speech: implement STT backend hot-swap with graceful rollback

## Purpose
Speech subsystem — local STT and TTS. Sena's primary user-facing interaction surface per P9 (speech-first). Supports multiple STT backends (Whisper via stt-worker, Sherpa-onnx, Parakeet) and platform TTS (with Piper planned). Includes wakeword detection and listen mode.

## Public API Surface
**Types:**
- `SttActor` — speech-to-text actor
- `TtsActor` — text-to-speech actor
- `WakewordActor` — wakeword detection
- `SpeechError` — error enum
- `AudioBuffer` — PCM samples
- `SttBackend` — Whisper | Sherpa | Parakeet | Mock (derives: Clone, Debug, PartialEq, Eq, Serialize, Deserialize with lowercase serde rename)
- `SttBackendHandle` — Whisper(WorkerHandle) | Sherpa(tx) | Parakeet(tx) | Mock (worker thread pattern for !Send backends)
- `ParakeetStt` — Parakeet-EOU streaming STT backend wrapper
- `SherpaZipformerStt` — Sherpa-onnx Zipformer Transducer streaming STT backend wrapper
- `TtsBackend` — Piper | SystemPlatform | Mock

**Functions:**
- `list_input_devices()` — microphone enumeration

**Modules:**
- `audio_input` — microphone capture
- `audio_output` — audio playback
- `error` — error types
- `models` — model definitions
- `onboarding` — first-enable flow
- `parakeet_stt` — Parakeet-EOU streaming STT backend
- `sherpa_stt` — Sherpa-onnx Zipformer streaming STT backend
- `silence_detector` (private) — VAD
- `stt` — STT implementation
- `stt_actor` — SttActor
- `telemetry` — CSV-based STT telemetry logging for A/B backend testing
- `tts_actor` — TtsActor
- `wakeword` — wakeword detection

## Bus Events Owned
Emits (defined in bus):
- `SpeechEvent::SpeechOutputCompleted`
- `SpeechEvent::SpeechFailed`
- `SpeechEvent::TranscriptionCompleted`
- `SpeechEvent::TranscriptionWordReady`
- `SpeechEvent::LowConfidenceTranscription`
- `SpeechEvent::VoiceInputDetected`
- `SpeechEvent::ListenModeTranscription`
- `SpeechEvent::ListenModeStopped`
- `SpeechEvent::WakewordDetected`
- `SpeechEvent::WakewordSuppressed`
- `SpeechEvent::WakewordResumed`
- `SpeechEvent::SttTelemetryUpdate` — backend performance metrics for A/B testing

Subscribes to:
- `SpeechEvent::SpeakRequested` — TTS
- `SpeechEvent::ListenModeRequested`
- `SpeechEvent::ListenModeStopRequested`
- `SpeechEvent::SttBackendSwitchRequested` — hot-swap STT backend
- `InferenceEvent::InferenceSentenceReady` — streaming TTS
- `InferenceEvent::InferenceStreamCompleted`
- `SystemEvent::LoopControlRequested`

## Dependency Edges
Imports from Sena crates: bus (only)
Imported by Sena crates: runtime
Key external deps:
- cpal (v0.15) — audio I/O
- tts (v0.26) — platform TTS
- tokio (async, features: fs, process, io-util) — process spawning for stt-worker
- serde_json — worker IPC protocol
- parakeet-rs (v0.3.4) — Parakeet-EOU streaming STT (ONNX, 120M params)
- sherpa-onnx (v1.12.36, shared feature) — Sherpa-onnx Zipformer Transducer streaming STT (replaces deprecated sherpa-rs)
- tracing

Child process dependency:
- stt-worker binary (whisper-rs isolated) — STT via stdin/stdout IPC

## Background Loops Owned
- `speech` — STT capture and wakeword detection

## Known Issues
- TODO: Pin real SHA-256 checksum for models
- TODO: M6 — expose pause/resume API for wakeword

## Backend Hot-Swap
**Runtime Backend Switching:**
- Triggered by `SpeechEvent::SttBackendSwitchRequested { backend: String }`
- Pattern: shutdown → init → broadcast result
- `backend_switching` flag pauses audio processing during switch
- Rollback-on-failure: if new backend init fails, reverts to old backend and re-initializes
- Emits `SttBackendSwitchCompleted { backend }` on success
- Emits `SttBackendSwitchFailed { backend, reason }` on failure
- Helper functions:
  - `parse_backend_name(backend_str)` — converts string to `SttBackend` enum (whisper | sherpa | parakeet | mock)
  - `shutdown_backend(handle)` — gracefully stops Whisper/Sherpa/Parakeet worker threads

## Telemetry
**CSV Logging (for A/B backend testing):**
- Platform-specific log path:
  - Windows: `%APPDATA%\sena\logs\stt_telemetry.csv`
  - macOS: `~/Library/Application Support/sena/logs/stt_telemetry.csv`
  - Linux: `~/.config/sena/logs/stt_telemetry.csv`
- CSV format: `backend,chunk_duration_ms,latency_ms,confidence,vram_mb`
- Auto-writes header on first log entry
- Logged per transcription event in SttActor
- Write failures are non-fatal (warn + continue)
- Backend VRAM estimates:
  - Whisper: 142MB (tiny.en quantized)
  - Sherpa: 100MB (Zipformer int8 estimate)
  - Parakeet: 480MB (realtime_eou_120m-v1 ONNX)
- Emits `SttTelemetryUpdate` bus event on each log entry

## Notes
**STT Pipeline (Multi-Backend Architecture):**

1. **Whisper (Worker Process):**  
   Microphone → cpal → VAD → PCM chunks → stt-worker stdin → whisper-rs → JSON events stdout → TranscriptionCompleted
   - Input: length-prefixed PCM chunks (4-byte little-endian length + raw PCM i16 samples)
   - Output: newline-delimited JSON events (listening/word/completed/error)
   - Worker lifecycle: spawn on first transcription request, graceful shutdown on actor drop
   - Auto-restart: exponential backoff on worker crash (emits SpeechUnavailable)
   - Process isolation: whisper-rs never linked into main Sena binary (fixes GGML conflict)

2. **Parakeet (Threaded Worker):**  
   Microphone → cpal → VAD → PCM chunks → mpsc channel → ParakeetStt worker thread → oneshot reply → TranscriptionCompleted
   - Worker thread: tokio::task::spawn_blocking with parakeet-rs ParakeetEOU model
   - Input: i16 PCM samples (converted to f32 normalized -1.0 to 1.0)
   - Output: transcribed text via oneshot reply
   - Model: NVIDIA Parakeet realtime_eou_120m-v1 ONNX (~480MB)
   - Source: HuggingFace altunenes/parakeet-rs realtime_eou_120m-v1-onnx

3. **Sherpa (Threaded Worker):**  
   Microphone → cpal → VAD → PCM chunks → mpsc channel → SherpaZipformerStt worker thread → oneshot reply → TranscriptionCompleted
   - Worker thread: std::thread::spawn (sherpa-onnx types are !Send, cannot use spawn_blocking)
   - Input: f32 PCM samples (converted from i16 via normalization)
   - Output: transcribed text via oneshot reply
   - Model: Sherpa-onnx Zipformer Transducer (~253MB fp32 or ~71MB int8)
   - API: OnlineRecognizer + OnlineStream from sherpa-onnx 1.12.36 (official crate, replaces deprecated sherpa-rs)
   - Model files: encoder.onnx, decoder.onnx, joiner.onnx, tokens.txt
   - Note: sherpa-onnx types (!Send) require dedicated worker thread pattern — cannot share tokio async context

**TTS Pipeline:**
SpeakRequested → platform TTS → cpal playback → SpeechOutputCompleted

**Streaming TTS Queue:**
1. InferenceSentenceReady → spawn synthesis task
2. BTreeMap<sentence_index, SynthResult> for ordering
3. Play from next_play_index upward
4. Queue depth bounded (default 5)
5. InferenceStreamCompleted drains queue

**Listen Mode:**
- /listen → ListenModeRequested → continuous capture
- Independent SilenceDetector (no cross-mode contamination)
- Wakeword suppressed during listen mode

**Wakeword:**
- Energy-based placeholder (real model deferred to BF.1)
- Disabled by default
- Debounce prevents rapid-fire

**Hard rules:**
- Never captures/stores raw audio persistently
- Never sends text to external services
- Wakeword model <= 20MB, idle CPU < 1%
- Speech failure doesn't affect CTP/inference/memory
- Never write partial sentence to Soul/memory
