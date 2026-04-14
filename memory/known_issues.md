# Known Issues
Last updated: 2026-04-12

## Build Errors
None — `cargo check --workspace` passes with 0 errors.

## Resolved Build Errors
**RESOLVED: GGML Symbol Conflict (2026-04-11)**
- Previously: `cargo test --workspace` failed with LNK2005 duplicate symbol errors
- Root cause: Both `llama_cpp_sys_2` (from infer) and `whisper_rs_sys` linked their own `ggml.c.obj`
- Resolution: Isolated whisper-rs to dedicated `stt-worker` binary. Speech actor spawns worker as child process with stdin/stdout IPC.
- Commit: 15311ed

## TODO Items in Production Code

| File:Line | TODO Text |
|---|---|
| [crates/inference/src/actor.rs#L1385](crates/inference/src/actor.rs#L1385) | Phase 7B: switch to RichSummaryRequested for section-based prompt assembly |
| [crates/inference/src/actor.rs#L1465](crates/inference/src/actor.rs#L1465) | M6: replace with memory::WorkingMemory for per-cycle token budget tracking |
| [crates/runtime/src/hardware_profile.rs#L186](crates/runtime/src/hardware_profile.rs#L186) | M1.5: macOS does not expose used VRAM via ioreg. Return 0 for now. |
| [crates/bus/src/events/system.rs#L62](crates/bus/src/events/system.rs#L62) | M6: wire to file-watch notification or IPC command. |
| [crates/runtime/src/download_manager.rs#L69](crates/runtime/src/download_manager.rs#L69) | Pin real SHA-256 checksum — placeholder skips verification |
| [crates/soul/src/actor.rs#L826](crates/soul/src/actor.rs#L826) | M6: implement full export (event log + identity signals → JSON). |
| [crates/speech/src/models.rs#L50](crates/speech/src/models.rs#L50) | Pin real SHA-256 checksum from HuggingFace |
| [crates/speech/src/wakeword.rs#L78](crates/speech/src/wakeword.rs#L78) | M6: when a real wakeword model is used, expose a pause/resume API |
| [crates/platform/src/linux.rs#L34](crates/platform/src/linux.rs#L34) | M1.5: implement via x11rb / atspi |
| [crates/platform/src/linux.rs#L70](crates/platform/src/linux.rs#L70) | M1.5: implement via X11 XGetImage and Wayland wl_shm |
| [crates/platform/src/linux.rs#L75](crates/platform/src/linux.rs#L75) | M5.1: implement via x11rb or Wayland on Linux |
| [crates/platform/src/macos.rs#L34](crates/platform/src/macos.rs#L34) | M1.5: implement via core-graphics / Accessibility API |
| [crates/platform/src/macos.rs#L133](crates/platform/src/macos.rs#L133) | M5.1: implement via ScreenCaptureKit on macOS |

## unwrap() in Production Paths
Most `unwrap()` calls are in test code (`#[cfg(test)]` modules). Production path exceptions:

| File:Line | Context |
|---|---|
| [crates/runtime/src/tray.rs#L323](crates/runtime/src/tray.rs#L323) | `tokio::runtime::Runtime::new().unwrap()` — tray callback, unavoidable |
| [crates/runtime/src/tray.rs#L333](crates/runtime/src/tray.rs#L333) | `tokio::runtime::Runtime::new().unwrap()` — tray callback |
| [crates/runtime/src/tray.rs#L343](crates/runtime/src/tray.rs#L343) | `tokio::runtime::Runtime::new().unwrap()` — tray callback |

Note: tray.rs unwrap() calls are in synchronous OS callback context where Result propagation is not possible. These are known limitations.

## NEEDS_HUMAN Items
None

## Cross-Repo Notes
- `infer` crate (ontic-ai/infer) is at v0.1.1
- `ech0` crate (kura120/ech0) is at v0.1.2
- GGML conflict may require coordination with infer and whisper-rs maintainers

## Platform-Specific Issues
- **Linux:** active_window, screen_capture TODOs (stubs returning None)
- **macOS:** active_window, screen_capture TODOs (stubs returning None)
- **macOS:** VRAM monitoring returns 0 (ioreg doesn't expose this)
- **Windows:** All features implemented
