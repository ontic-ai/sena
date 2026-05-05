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

**Build from the canonical nested workspace:**

```sh
git clone https://github.com/ontic-ai/sena.git
cd sena/sena              # ← Enter the nested workspace directory
rustup override set $(cat rust-toolchain.toml | grep channel | cut -d' ' -f3 | tr -d '"')
cargo build --workspace
cargo test --workspace
```

> ⚠️ **Do NOT** run `cargo build` from the root without specifying a package. The root `Cargo.toml` is a deprecated donor workspace. Always use `sena/` (nested workspace, Rust 2024).

## Useful links

- Architecture: `docs/architecture.md`
- Roadmap: `docs/ROADMAP.md`
- Product requirements: `docs/PRD.md`
- Contribution rules: `CONTRIBUTING.md`

## Contribute

1. Read `CONTRIBUTING.md`.
2. Use `.github/ISSUE_TEMPLATE` for bug reports and feature requests.
3. Use `.github/pull_request_template.md` for PRs.

## License

MIT © 2026 Ontic
