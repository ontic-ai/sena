# Crate: cli
Path: crates/cli/
Last updated: 2026-04-12
Last commit touching this crate: 5f95a24 cli: add /stt-backend command for runtime backend switching

## Purpose
Binary entrypoint — thin shell only. The CLI is a wrapper over the daemon's capabilities, NOT an owner of business logic. It dispatches typed bus events to request work and renders responses via ratatui TUI. In Phase 6+, CLI connects to daemon over IPC.

## Public API Surface
**Binary:** `sena`
- No args → daemon mode (`runtime::run_background()`)
- `cli` arg → TUI mode (IPC connection or auto-start daemon)

## Modules
- `main` — entry point, logging setup
- `shell` — TUI REPL, slash commands, event rendering
- `display` — ratatui widget rendering
- `ipc_client` — IPC connection to daemon
- `model_selector` — model selection UI
- `onboarding` — first-boot wizard
- `query` — transparency command handlers
- `tui_state` — TUI state machine

## Bus Events Owned
None — CLI dispatches events, does not own them

## Dispatches to daemon:
- `/help`, `/clear`, `/exit`, `/quit`
- `/status`, `/models`, `/model switch`
- `/observation`, `/memory [query]`, `/explanation`
- `/loops [name] [on|off]`
- `/config`, `/config set <key> <value>`
- `/listen`, `/wakeword [on|off]`, `/stt-backend [backend]`
- `/voice [enable|disable]`, `/microphone [list|select]`
- `/verbose [on|off]`
- User chat messages → InferenceRequested

**Command Details:**
- `/stt-backend` — show current STT backend (whisper/sherpa/parakeet)
- `/stt-backend <backend>` — switch STT backend at runtime
  - Guards against switching during active `/listen` session
  - Case-insensitive backend name validation
  - Event handlers for SttBackendSwitchCompleted/Failed display results

## Dependency Edges
Imports from Sena crates:
- runtime (boot, config, model discovery)
- bus (events, IpcMessage)

May NOT import: soul, platform, ctp, memory, inference, prompt, speech, crypto

Key external deps:
- ratatui (v0.30) — TUI
- crossterm (v0.28) — terminal control
- arboard — clipboard
- anyhow — error handling (allowed only in CLI)
- tracing-subscriber, tracing-appender — logging
- toml, serde_json

## Background Loops Owned
None — event-driven rendering

## Known Issues
None in production paths

## Notes
**CLI Design Principle — Wrapper, Not Owner:**
- CLI dispatches events, renders responses
- All business logic lives in daemon actors
- CLI never constructs actors
- CLI crash does not affect runtime

**Phase 6+ IPC:**
- If daemon running: connect via IPC
- If daemon not running: auto-start, then connect
- `run_with_runtime()` path removed

**Logging:**
- File: INFO+ to `<config_dir>/sena/sena.<date>.log`
- Stderr: debug builds or SENA_LOG_STDERR=1
- TUI mode passes allow_stderr=false to avoid corruption

**Phase 7A UX Polish:**
- /help grouped by category
- Input length limit (4096 chars) with counter
- No-match autocomplete indicator
- Model loading feedback
- Formatted inference errors
- Enhanced onboarding

**Slash command mapping:**
Every command maps 1:1 to an IPC command or bus event.
No orphaned CLI commands.
