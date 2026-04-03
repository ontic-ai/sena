# Contributing to Sena

Thanks for helping build Sena! This project follows strict architecture and security rules in `docs/architecture.md` and `.github/copilot-instructions.md`. Please read them before writing code.

## Quick-start (local dev only)

1. Install Rust (stable release matching `rust-toolchain.toml`).
2. Clone repository and enter:
   ```sh
   cd sena
   ```
3. Build:
   ```sh
   cargo build --workspace
   ```
4. Test:
   ```sh
   cargo test --workspace
   ```
5. Run:
   ```sh
   cargo run
   ```

> ⚠️ Not for production use yet. Local builds are for contributors only.

## Branching and PRs

- Branch name: `feature/<short-name>`, `fix/<short-name>`, or `chore/<short-name>`.
- One logical change per PR.
- Include issue link and test commands in PR description.

## Code style and checks

- No `unwrap()` in production code.
- No direct cross-crate imports violating `docs/architecture.md` dependency direction.
- `tokio` runtime only; no `async-std`.
- Use `thiserror` in libs and `anyhow` only in `crates/cli`.

Run before pushing:

```sh
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
``` 

## Testing guidelines

- Every public function must have at least one test.
- Use `tempfile::tempdir()` for filesystem tests.
- No network calls in tests.
- Use model mocks for inference tests; avoid real GGUF files.

## Reporting issues

Use the issue templates under `.github/ISSUE_TEMPLATE/`.

## Community

- Follow the code of conduct in `CODE_OF_CONDUCT.md`.
