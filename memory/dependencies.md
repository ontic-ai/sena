# Dependency Registry
Last updated: 2026-04-12
Auto-maintained by memory-keeper after every commit.

## External Dependencies

| Crate | Version Pinned | Used By | Purpose |
|-------|---------------|---------|---------|
| tokio | 1.41 (features = ["full"]) | all crates | Async runtime |
| thiserror | 2.0 | all lib crates | Error derive macro |
| anyhow | 1.0 | cli only | Error handling in binary |
| serde | 1.0 (features = ["derive"]) | bus, runtime, inference | Serialization |
| async-trait | 0.1 | actors | Async trait support |
| toml | 1.1.2 | runtime, cli | Config parsing |
| zeroize | 1.8 (features = ["derive"]) | crypto | Secure memory zeroing |
| rand | 0.9 | crypto, runtime | Random generation |
| tracing | 0.1 | all crates | Structured logging |
| tracing-subscriber | 0.3 (features = ["env-filter"]) | cli | Log subscriber init |
| tracing-appender | 0.2 | cli | Log file rotation |
| aes-gcm | 0.10 | crypto | AES-256-GCM encryption |
| argon2 | 0.5 | crypto | Key derivation |
| keyring | 3 | crypto | OS keychain access |
| redb | 3.1.2 | soul | Embedded database |
| infer | v0.1.1 (git) | inference, bus | LLM inference backend |
| ech0 | v0.1.2 (git) | memory | Memory graph + vector store |
| arboard | 3 | platform, cli | Clipboard access |
| rdev | 0.5 | platform | Keystroke timing |
| notify | 8.2.0 | platform | File system events |
| ratatui | 0.30 (features = ["crossterm"]) | cli | Terminal UI |
| crossterm | 0.28 | cli | Terminal control |
| windows-sys | 0.61.2 | platform (Windows) | Windows API |
| core-graphics | 0.24 | platform (macOS) | macOS screen capture |
| cpal | 0.15 | speech | Audio I/O |
| tts | 0.26 | speech | Platform TTS |
| whisper-rs | 0.16.0 | stt-worker | Whisper STT (isolated binary) |
| sherpa-onnx | 1.12.36 (features = ["shared"]) | speech | Sherpa-onnx Zipformer Transducer streaming STT (ONNX, ~253MB fp32 or ~71MB int8, Apache-2.0, replaces deprecated sherpa-rs) |
| parakeet-rs | 0.3.4 | speech | Parakeet-EOU streaming STT (ONNX, 120M params, MIT/Apache-2.0) |
| candle-core | 0.10 | speech | Candle ML framework (Whisper) |
| candle-nn | 0.10 | speech | Candle neural network ops |
| candle-transformers | 0.10 | speech | Candle transformers (Whisper) |
| tokenizers | 0.21 (features = ["onig"]) | speech | Tokenizer for Whisper |
| tray-icon | 0.14 | runtime | System tray |
| reqwest | 0.12 (features = ["rustls-tls", "stream"]) | runtime | Model downloads |
| sha2 | 0.10 | runtime, platform | Checksum verification |
| sysinfo | 0.32 | runtime, platform | System info |
| image | 0.25 (features = ["png"]) | runtime, platform | Image processing |
| uuid | 1 (features = ["v4"]) | inference, memory | UUID generation |
| chrono | 0.4 | memory | Timestamps |
| tempfile | 3 | dev-dependencies | Temp dirs in tests |
| serde_json | 1 | runtime, cli, memory | JSON (de)serialization |
| futures-util | 0.3 | runtime | Stream utilities |
| hex | 0.4 | runtime | Hex encoding |

## Git Dependencies (external repos)
| Crate | Repository | Tag | Purpose |
|-------|-----------|-----|---------|
| infer | github.com/ontic-ai/infer | v0.1.1 | LLM backend (llama-cpp-2 wrapper) |
| ech0 | github.com/kura120/ech0 | v0.1.2 | Memory graph + vector store |

## Banned Crates
| Crate | Reason |
|---|---|
| reqwest (general HTTP) | Used ONLY for model downloads per P1 exception |
| openai / any cloud AI SDK | Violates P1 (local-first) |
| rusqlite / sqlite / sqlx | Replaced by redb (Soul) and ech0 (memory) |
| lazy_static | Use std::sync::OnceLock |
| failure | Superseded by thiserror |
| Any crate that calls process::exit | Shutdown is runtime's job |

## Dependency Evaluation Criteria
Before adding any dependency:
1. Is there a std solution good enough?
2. Is crate actively maintained (commit in last 6 months)?
3. Does crate have no_std option if needed later?
4. Does crate compile on macOS, Windows, Linux?
