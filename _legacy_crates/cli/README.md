# Sena CLI

The `sena` binary boots the Sena runtime and drops you into an interactive REPL. All commands are slash-prefixed. There is no need for a second terminal.

---

## First Run

On first launch Sena automatically generates a master key and stores it in the OS keychain (Windows Credential Manager / macOS Keychain / Secret Service). No passphrase prompt. No env vars required.

```powershell
cargo run
```

---

## Running Sena

**During development** — arguments go after `--`:

```powershell
cargo run               # interactive REPL (default)
cargo run -- models     # standalone model picker, then exit
cargo run -- query obs  # scripting: single query, print, exit
```

> `cargo run sena` is wrong — it passes `sena` as an argument. Put arguments after `--`.

**After installing:**

```powershell
cargo install --path crates/cli
sena            # interactive REPL
sena models     # standalone model picker
sena query obs  # single query
```

---

## Interactive REPL

Default mode. Boots the runtime once and stays running. Queries execute against the live runtime — no re-boot per command.

```
  ╔══════════════════════════════════╗
  ║       · S E N A ·                ║
  ║       local-first ambient AI     ║
  ╚══════════════════════════════════╝

sena › _
```

Press `Ctrl+C` at any time to exit. Actors shut down gracefully.

### Command Reference

| Command | Alias | What it does |
|---------|-------|--------------|
| `/observation` | `/obs` | What is Sena observing right now? |
| `/memory` | `/mem` | What does Sena remember about you? |
| `/explanation` | `/why` | Why did Sena say that? (last inference) |
| `/models` | | Select which Ollama model to use |
| `/help` | `/h` | Show command reference |
| `/quit` | `/q` `/exit` | Exit Sena |

---

## `/obs` — Current Observation

Asks the CTP actor for what Sena is seeing: active window, inferred task, clipboard state, keystroke rate.

```
sena › /obs
  ·  Querying...

  ━━  Current Observation

  Window        Visual Studio Code — main.rs
  Task          Writing Rust (87%)
  Clipboard     ✓ ready
  Keyboard      142 events/min
  Session       5 min 42 sec
```

---

## `/mem` — Memory

Asks the memory actor for your Soul summary (long-term work patterns) and the most recent ech0 memory chunks.

```
sena › /mem

  ━━  Memory

  Work patterns  Rust development, CLI tooling
  Tools          VS Code, PowerShell, Cargo
  Interests      systems programming, AI

  Recent memories
  [1]  Implemented model_selector.rs for CLI
       score: 0.91
  [2]  Discussed architecture dependency rules
       score: 0.84
```

---

## `/why` — Last Inference Explanation

Shows the last inference the model completed: request context, response, and working memory chunks used.

```
sena › /why

  ━━  Last Inference

  Rounds    1
  Request   [context: VS Code open, writing Rust CLI] [...]
  Response  You appear to be implementing a CLI subcommand...
  Memory    2 chunk(s) used
            [1] query.rs implementation context
            [2] CLI architecture rules discussion
```

---

## `/models` — Model Selector

Scans your Ollama directory, shows a numbered table, and saves the selection to `config.toml`.

```
sena › /models
  ·  Scanning: C:\Users\you\.ollama\models
  ────────────────────────────────────────
  [1]  mistral:latest      4.1 GB   Q4_0
  [2]  llama3:8b           4.7 GB   Q4_0  ←
  [3]  phi3:mini           2.3 GB   Q4_0
  ────────────────────────────────────────
  ·  Currently selected: llama3:8b
  > Enter number or model name (Enter to keep current):
```

Accepted input: a number (`2`), a model name (`llama3:8b`), or empty Enter to keep the current selection. The change takes effect on the next boot.

**Ollama model directory:**

| OS | Path |
|----|------|
| Windows | `%USERPROFILE%\.ollama\models\` |
| macOS | `~/.ollama/models/` |
| Linux | `~/.ollama/models/` |

If no models are found, install [Ollama](https://ollama.com) and run `ollama pull mistral`.

---

## Scripting Mode

Bypass the REPL for one-shot automation:

```powershell
cargo run -- query observation   # prints observation, exits 0
cargo run -- query memory        # prints memory, exits 0
cargo run -- query explanation   # prints last inference, exits 0
```

Boots a short-lived runtime, sends the query, prints the result, exits. Timeout: 5 seconds.

---

## Config File

Created automatically on first run.

| OS | Path |
|----|------|
| Windows | `%APPDATA%\sena\config.toml` |
| macOS | `~/Library/Application Support/sena/config.toml` |
| Linux | `~/.config/sena/config.toml` |

The `preferred_model` key is written by `/models`. Example:

```toml
preferred_model = "llama3:8b"
ctp_trigger_interval_secs = 300
shutdown_timeout_secs = 5
clipboard_observation_enabled = true
```

---

## Getting a Model

```powershell
ollama pull mistral      # 4 GB, good all-around
ollama pull llama3:8b   # 4.7 GB, strong reasoning
ollama pull phi3:mini   # 2.3 GB, fast on CPU
```

Then use `/models` inside the REPL (or `cargo run -- models`) to select one.

---

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Error — message printed to stderr |

Scripting queries (`cargo run -- query ...`) exit `1` if no response arrives within 5 seconds.
