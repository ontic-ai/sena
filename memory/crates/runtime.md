# Crate: runtime
Path: crates/runtime/
Last updated: 2026-04-12
Last commit touching this crate: ca934c6 — runtime: add stt_backend config field for boot-time backend selection

## Purpose
Boot sequence, actor registry, shutdown orchestration, and process lifetime management. This is the composition root — it constructs ALL concrete actor instances. Owns the supervisor loop for daemon mode and provides entry points for both daemon and CLI modes.

## Public API Surface
**Entry points:**
- `run_background()` — boot + readiness gate + supervision loop (daemon mode)
- `boot_ready()` — boot + readiness gate, returns `Runtime` (CLI mode)
- `boot()` — raw boot sequence

**Types:**
- `Runtime` — runtime state (bus, expected_actors, actor handles)
- `BootError` — boot failure
- `RuntimeError` — unified runtime error
- `SenaConfig` — configuration structure (includes `stt_backend: SttBackend` field, default: Whisper)
- `ConfigError` — config parsing errors
- `ActorRegistry` — actor tracking
- `ModelRegistry` — model discovery
- `TrayManager` — system tray
- `SingleInstanceGuard` — single instance lock
- `ModelManifest` — download manifest

**Functions:**
- `discover_models()` — find GGUF models
- `ollama_models_dir()` — Ollama model path
- `save_config()` — persist config
- `shutdown()` — graceful shutdown
- `wait_for_sigint()` — SIGINT handler
- `suppress_llama_logs()` — silence llama.cpp output
- `is_first_boot()` — check for soul.redb.enc
- `is_daemon_running()` — check for running instance
- `try_acquire_lock()` — single instance lock
- `list_input_devices()` — microphone enumeration (re-export from speech)

**Modules:**
- `boot` — boot sequence
- `config` — TOML config
- `supervisor` — readiness gate, supervision loop
- `tray` — system tray
- `shutdown` — shutdown protocol
- `registry` — actor registry
- `models` — model discovery
- `ipc_server` — IPC server for CLI communication
- `download_manager` — model downloads
- `analytics` — local usage analytics
- `hardware_profile` — RAM/VRAM detection
- `single_instance` — single instance enforcement

## Bus Events Owned
None directly — runtime subscribes to SystemEvent and coordinates actors

## Dependency Edges
Imports from Sena crates:
- bus (events, Actor trait)
- crypto (encryption init)
- soul (SoulActor)
- platform (PlatformActor)
- ctp (CTPActor)
- memory (MemoryActor)
- inference (InferenceActor)
- speech (SttActor, TtsActor, WakewordActor)

Imported by Sena crates: cli

Key external deps:
- tokio (runtime)
- reqwest (model downloads)
- sysinfo (hardware detection)
- tray-icon (system tray)
- sha2 (checksums)
- image (tray icon)

## Background Loops Owned
- `vram_monitor` — polls GPU VRAM every 10s

## Known Issues
- TODO: macOS VRAM monitoring returns 0 (ioreg limitation)
- TODO: Pin real SHA-256 checksums for model downloads

## STT Backend Configuration
**Boot-time backend selection:**
- `SenaConfig.stt_backend` field specifies which STT backend to use (default: Whisper)
- TOML format: `stt_backend = "whisper"` (or "sherpa", "parakeet", "mock")
- CLI command: `/config set stt_backend <backend>` (applies to config.toml and next boot)
- SttActor initialization uses `config.stt_backend` value (no hardcoded backend)
- Runtime hot-swap available via `SpeechEvent::SttBackendSwitchRequested`

## Notes
- Boot order is strict (steps 1–12 in architecture.md §4.1)
- Readiness gate: 30s timeout for ActorReady from all expected_actors
- BootComplete emitted AFTER readiness gate
- Supervisor restarts failed actors up to 3 times
- Shutdown timeout: 5s per actor (configurable)
