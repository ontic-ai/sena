# Crate: stt-worker
Path: crates/stt-worker/
Last updated: 2026-04-11
Last commit touching this crate: 15311ed — speech: isolate whisper-rs to stt-worker binary to fix GGML symbol conflict

## Purpose
Standalone binary that isolates whisper-rs from the main Sena process. Spawned as a child process by the speech actor. Eliminates GGML symbol conflict between llama_cpp_sys_2 (inference crate) and whisper_rs_sys (speech crate).

## Binary Type
**Standalone executable** — not a library. Never imported by any Sena crate. Invoked via `tokio::process::Command`.

## Communication Protocol
**IPC via stdin/stdout**

### Input (stdin)
Length-prefixed PCM chunks:
- 4 bytes: chunk length (u32, little-endian)
- N bytes: raw PCM samples (i16 interleaved, 16kHz mono)

### Output (stdout)
Newline-delimited JSON events:

```json
{"type": "listening"}
{"type": "word", "text": "hello", "confidence": 0.95}
{"type": "completed", "text": "hello world", "confidence": 0.92}
{"type": "error", "message": "model load failed"}
```

## Lifecycle
1. **Spawn:** `speech::worker::Worker::spawn()` forks the binary
2. **Initialize:** Worker loads Whisper model on first stdin chunk
3. **Run:** Main loop reads stdin, processes audio, writes JSON to stdout
4. **Shutdown:** EOF on stdin triggers graceful cleanup and exit
5. **Crash:** Speech actor detects broken pipe, emits `SpeechUnavailable`, restarts with exponential backoff

## Dependency Edges
Imports from Sena crates: **NONE** — fully isolated
Imported by Sena crates: **NONE** — process-level isolation
Key external deps:
- whisper-rs (v0.16.0) — Whisper STT (exclusive owner of whisper_rs_sys)
- serde + serde_json — JSON event serialization

## Hard Rules
- **NEVER** add this crate to any other crate's Cargo.toml dependencies
- **NEVER** import any Sena lib crate (bus, runtime, etc.) — this breaks the isolation boundary
- **NEVER** link whisper-rs into any other Sena crate — stt-worker is the sole owner
- All communication is stdin/stdout — no shared memory, no IPC channels beyond pipes

## Architecture Rationale
whisper-rs and llama-cpp-2/infer both transitively depend on ggml.c, but each vendored their own copy. Linking both into the same binary causes duplicate symbol errors (LNK2005 on Windows). Process isolation ensures the symbols never coexist in the same address space.

## Notes
- Model path is read from an environment variable (set by speech actor)
- Worker is designed to be stateless per invocation — each PCM chunk is independent
- Worker emits incremental `word` events for streaming feedback (displayed in CLI listen mode)
- Worker does NOT use tokio — synchronous I/O only (blocking read/write on stdin/stdout)
- Exit code 0 = clean shutdown, non-zero = error (logged by speech actor)
