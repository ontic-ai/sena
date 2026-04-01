# Testing Phase 4 M4.3 Transparency UI

## How to Test Interactive Queries

After building Sena with `cargo build --workspace`, you can test the transparency UI with these commands:

### 1. Query Current Observation
```bash
cargo run --release -- query observation
```

**Expected Output:**
```
Current Context: [app name] | [task] | [clipboard ready/no clipboard] | [keystroke rate] | [session duration]
```

Shows what Sena is observing right now - the active application, inferred task, clipboard status, keystroke cadence, and how long the session has been running.

### 2. Query User Memory
```bash
cargo run --release -- query memory
```

**Expected Output:**
```
Soul Summary: patterns=[morning_coder,...] | preferences=[vscode,...] | interests=[rust,...]
Recent Memories:
  [memory chunk text] (score: 0.92)
  [memory chunk text] (score: 0.87)
  ...
```

Shows what Sena has learned about you - your work patterns, tool preferences, and interests extracted from inference cycles. Also displays the most relevant recent memories ranked by score.

### 3. Query Inference Explanation
```bash
cargo run --release -- query explanation
```

**Expected Output:**
```
Last Inference (Rounds: 1):
Request: Inference request about [context]
Response: [First 200 chars of response]...
Working Memory: 3 chunks
```

Shows why Sena said what it said in the last inference cycle - the reasoning context, the response generated, and which memory chunks were available in working memory.

## Architecture

The implementation uses typed event-based communication:

1. **Bus Events** (crates/bus/src/events/transparency.rs):
   - `TransparencyQuery` enum (CurrentObservation, UserMemory, InferenceExplanation)
   - Response types for each query type
   - `SoulSummaryForTransparency` (redacted soul state)

2. **Actor Handlers**:
   - CTP Actor: Responds to CurrentObservation with current ContextSnapshot
   - Memory Actor: Responds to UserMemory with soul summary + memory chunks
   - Inference Actor: Responds to InferenceExplanation with last inference state

3. **CLI Interface** (crates/cli/src/query.rs):
   - Parses query type from command line
   - Boots runtime and sends query via bus
   - Awaits response with 5-second timeout
   - Displays formatted output

## Security & Privacy

- No raw keystroke characters or clipboard text exposed (only digests and cadence)
- SoulSummaryForTransparency redacts identity data to high-level aggregates
- All encrypted stores (ech0, soul redb) remain encrypted and isolated
- Working memory context is safe to expose (already redacted at collection time)
- Tested and approved by security audit (CLEAN verdict)

## Implementation Status

✅ All five units complete
✅ Build passes
✅ Tests pass (280+)
✅ Clippy clean
✅ Format correct
✅ Security approved
✅ Architecture approved
✅ Committed and pushed to GitHub

Ready for user testing!

---

## M4.4 Longevity Testing

### 72-Hour Stability Test

M4.4 requires verifying that Sena runs for 72 hours without restart. A dedicated longevity test exists in the `runtime` crate.

**Test command:**
```bash
cargo test -p runtime --test stability longevity_72h -- --ignored --nocapture
```

**What it does:**
- Boots all actors with `MockBackend` (no GGUF model required)
- Sends inference requests every 2 seconds for 72 hours (259,200 seconds)
- Monitors memory usage every 60 seconds
- Reports progress every hour
- Asserts memory stays below 512 MB ceiling
- Asserts all requests receive responses (no message drops)
- Asserts no actor panics or exits early

**Duration:** Exactly 3 days (72 hours). This test is marked `#[ignore]` so it does NOT run in default CI.

**Why local-first compliant:**
- Uses `MockBackend` — no network LLM calls
- Uses `tempfile::tempdir()` for all persistent state
- No external dependencies or API calls
- All validation is in-process

**Expected output:**
```
[longevity] Starting 72-hour test. This will take 3 days.
[longevity] Press Ctrl+C to terminate early if needed.
[longevity] Progress: 1h elapsed, 71h remaining | requests=1800 responses=1800 peak_mem=128MB
[longevity] Progress: 2h elapsed, 70h remaining | requests=3600 responses=3600 peak_mem=145MB
...
[longevity] Progress: 72h elapsed, 0h remaining | requests=129600 responses=129600 peak_mem=384MB
[longevity] 72 hours complete. Shutting down actors...
[longevity] Final: duration=72h requests=129600 responses=129600 peak_memory=384MB
[longevity] ✓ All assertions passed. Sena survived 72 hours.
```

**Milestone verification:**
This test directly satisfies M4.4 exit gate requirement: *"Sena runs for 72 hours without restart in testing"*

**Quick validation (without waiting 72h):**
To verify the test compiles and registers correctly without running the full duration:
```bash
cargo test -p runtime --test stability longevity_72h -- --ignored --list
```
Should output: `longevity_72h_no_leak_no_panic: test`
