# Crate: platform
Path: crates/platform/
Last updated: 2026-04-11
Last commit touching this crate: (see git log)

## Purpose
OS adapter trait and per-OS signal collection implementations. This is the ONLY place OS-specific code lives in Sena. Collects active window, clipboard, file events, and keystroke timing patterns.

## Public API Surface
**Traits:**
- `PlatformAdapter` — OS signal collection interface

**Types:**
- `PlatformError` — platform operation errors
- `PlatformActor` — actor for signal emission

**Functions:**
- `create_platform_adapter()` — factory for current OS
- `config_dir()` — platform-specific config directory
- `ollama_models_dir()` — Ollama model path
- `detect_compute_backend()` — Metal/CUDA/CPU detection

**Modules:**
- `adapter` — PlatformAdapter trait
- `dirs` — directory utilities
- `error` — error types
- `factory` — adapter creation
- `platform_actor` — Actor implementation
- `linux` — Linux implementation
- `macos` — macOS implementation
- `windows` — Windows implementation

## Bus Events Owned
Emits (defined in bus):
- `PlatformEvent::WindowChanged`
- `PlatformEvent::ClipboardChanged`
- `PlatformEvent::FileEvent`
- `PlatformEvent::KeystrokePattern`
- `PlatformVisionEvent::VisionFrameReady`

## Dependency Edges
Imports from Sena crates: bus
Imported by Sena crates: runtime, ctp
Key external deps:
- arboard (clipboard)
- rdev (keystroke timing)
- notify (file events)
- sysinfo (system info)
- sha2 (digests)
- image (screen capture)
- windows-sys (Windows)
- core-graphics (macOS)

## Background Loops Owned
- `platform_polling` — polls active window, clipboard, keystrokes
- `screen_capture` — periodic screenshot acquisition

## Known Issues
**Linux TODOs:**
- active_window: returns None (implement via x11rb/atspi)
- screen_capture: returns None (implement via X11/Wayland)

**macOS TODOs:**
- active_window: returns None (implement via core-graphics/Accessibility)
- screen_capture: returns None (implement via ScreenCaptureKit)

**Windows:**
- All features implemented

## Notes
- Keystroke captures TIMING ONLY — characters never captured
- Clipboard content passed as digest — never stored verbatim
- File event scope configurable
- Platform guards: `#[cfg(target_os = "...")]`
- Every platform branch covered — no silent catch-alls
