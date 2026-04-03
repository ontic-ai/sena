# Sena

Ambient intelligence. Local-first. OS-native. Uniquely yours.

## Our Mission

An AI that understands you,
*without the cloud.*

Sena is your personal assistant that lives on your computer. It watches what you do, learns your habits, and helps you work smarter — all while keeping everything completely private. No data ever leaves your machine.

## What is Sena?

Sena is an open-source, Rust-based ambient intelligence framework built around the following principles:

- Local-first: no external network dependency for core inference and memory.
- Privacy-first: encrypted stores and a zero-data-leak architecture.
- Modular: actors for inference, memory, context, platform, and UI communicate via an event bus.

## Install (for contributors)

> ⚠️ Not for production yet. Intended for local development and contribution.

```sh
git clone https://github.com/ontic-ai/sena.git
cd sena
rustup override set $(cat rust-toolchain)
cargo build --workspace
cargo test --workspace
```

## Useful links

- Architecture: `docs/architecture.md`
- Roadmap: `docs/ROADMAP.md`
- Product requirements: `docs/PRD.md`
- Coding rules: `.github/copilot-instructions.md`

## Contribute

1. Read `CONTRIBUTING.md`.
2. Use `.github/ISSUE_TEMPLATE` for bug reports and feature requests.
3. Use `.github/pull_request_template.md` for PRs.

## License

MIT © 2026 Ontic
