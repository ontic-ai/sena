# Crate: bus
Path: crates/bus/
Last updated: 2026-04-11
Last commit touching this crate: a302d5e cli: add WakewordSuppressed/Resumed verbose handlers

## Purpose
The central nervous system of Sena. Provides the typed event bus, Actor trait, and all event type definitions. Every subsystem communicates through it — no subsystem calls another subsystem's functions directly. This is a leaf node in the dependency graph with zero Sena crate dependencies.

## Public API Surface
**Re-exports:**
- `Actor` trait — async actor lifecycle
- `ActorError` — actor failure enum
- `EventBus` — broadcast + mpsc routing
- `BusError` — bus operation errors
- `Event` — top-level event enum

**Event types:**
- `CTPEvent` — ThoughtEventTriggered, ContextSnapshotReady, UserStateComputed, SignalPatternDetected, EnrichedTaskInferred
- `DownloadEvent` — DownloadStarted, DownloadProgress, DownloadCompleted, DownloadFailed
- `InferenceEvent` — InferenceRequested, InferenceCompleted, InferenceStatusUpdate, streaming events
- `InferenceSource` — enum: UserVoice, UserText, ProactiveCTP, Iterative
- `MemoryEvent` — MemoryWriteRequest, MemoryQueryRequest, MemoryQueryResponse, ContextMemoryQuery*
- `PlatformEvent` — WindowChanged, ClipboardChanged, FileEvent, KeystrokePattern
- `PlatformVisionEvent` — VisionFrameReady, VisionFrameRequested
- `Priority` — inference priority levels
- `SoulEvent` — SoulSummaryReady, SoulEventLogged, intelligence events
- `SpeechEvent` — STT/TTS events, wakeword, listen mode
- `SystemEvent` — ShutdownSignal, BootComplete, ActorReady, ActorFailed, LoopControl*
- `TransparencyEvent` — user queries about what Sena observes/remembers
- `TrayMenuItem` — system tray integration

**IPC types:**
- `IpcMessage`, `IpcPayload`, `LineStyle`, `IPC_SCHEMA_VERSION`

## Bus Events Owned
ALL events are defined in this crate. Organized in `crates/bus/src/events/`:
- `ctp.rs` — CTP events
- `download.rs` — model download events
- `inference.rs` — inference pipeline events
- `memory.rs` — memory system events
- `platform.rs` — OS signal events
- `platform_vision.rs` — vision capture events
- `soul.rs` — SoulBox events
- `speech.rs` — STT/TTS events
- `system.rs` — lifecycle events
- `transparency.rs` — user query events

## Dependency Edges
Imports from Sena crates: (none — leaf node)
Imported by Sena crates: ALL crates depend on bus
Key external deps:
- tokio (channels)
- thiserror (error derive)
- async-trait (Actor trait)
- serde (serialization)
- infer (re-exports ModelInfo, Quantization)

## Background Loops Owned
None — bus is passive infrastructure

## Known Issues
None in production paths

## Notes
- Events are `Clone + Send + 'static`
- Events carry no logic — pure data
- Events are immutable once sent
- `InferenceSource` replaces `request_id < 1000` convention
