# Sena

This workspace is the official standalone rewrite of Sena.

The top-level directory is still named `baseplate/` only to keep `sena/sena` donor-only during the rewrite. It is a workspace boundary, not the product name.

- `apps/sena` wires the runtime and optional UI subscribers together.
- `crates/runtime` owns the headless runtime surface and Sena XML parsing.
- `crates/bus`, `crates/sri`, and `crates/speech` define the shared contracts the runtime will grow into.
- `crates/bootstrap` and `crates/cli` are visible subscriber surfaces and remain separate from the headless runtime.

The local planning artifact for this workspace lives in `docs/plan.md` and is intentionally git-ignored.
