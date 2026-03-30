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
