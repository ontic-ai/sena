# Sena Project Memory

## Key Rules
- Features MUST be plugged in after implementation — coded ≠ integrated
- All events must have both emitters AND handlers
- After implementing a feature, verify end-to-end flow works
- No dead code in production paths — if it's coded, it must be used
- Before claiming completion, trace from trigger → effect/persistence
- **Always update docs/PRD.md, docs/architecture.md, and docs/ROADMAP.md** when architectural decisions change. This is mandatory — omitting it is a protocol violation.

## Architecture (Current — Post-M-Refactor)
- `sena.exe` (no args) = daemon: `runtime::run_background()` → boot → readiness gate → BootComplete → supervision loop
- `sena.exe cli` = CLI TUI: `runtime::boot_ready()` → `shell::run_with_runtime()` → TUI → shutdown
- Tray "Open CLI" → `CliAttachRequested` → supervisor spawns new terminal running `sena cli`
- BootComplete is emitted AFTER supervisor readiness gate (all expected actors emitted ActorReady) — NOT in boot()
- Runtime owns process lifetime — CLI has no lifecycle responsibilities
- Expected actors: Soul, Platform, CTP, Memory, Inference; TTS+STT added if speech_enabled; Wakeword NOT added (no ActorReady)
