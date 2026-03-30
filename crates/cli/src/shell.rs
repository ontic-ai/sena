//! Interactive REPL shell for Sena — crossterm-powered TUI.
//!
//! Features:
//! - Alternate screen buffer (original terminal restored on exit)
//! - Raw mode for per-character input with / command dropdown
//! - Free-text chat: non-/ input is sent to the inference actor
//! - /verbose toggle: displays internal actor events as they arrive
//! - Auto-restart prompt after /models model selection

use std::io::{self, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bus::events::inference::{InferenceEvent, Priority};
use bus::events::transparency::TransparencyQuery;
use bus::Event;
use crossterm::{
    cursor,
    event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, ClearType},
};
use tokio::sync::broadcast;

use crate::{display, model_selector, query};
use runtime::boot::Runtime;

// ── ANSI fallback constants used in non-crossterm output paths ────────────────
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";

/// Reason the shell exited — drives the restart loop in main.rs.
#[derive(Debug, PartialEq)]
pub enum ShellExitReason {
    Quit,
    Restart,
}

/// Commands shown in the / dropdown.
const COMMANDS: &[(&str, &str)] = &[
    ("/observation", "What are you observing right now?"),
    ("/obs", "What are you observing right now?"),
    ("/memory", "What do you remember about me?"),
    ("/mem", "What do you remember about me?"),
    ("/explanation", "Why did you say that?"),
    ("/why", "Why did you say that?"),
    ("/models", "Select which Ollama model to use"),
    ("/verbose", "Toggle verbose actor-event logging"),
    ("/help", "Show all commands"),
    ("/quit", "Exit Sena"),
];

const PROMPT_PREFIX_LEN: usize = 7; // visual width of "sena › "

/// RAII guard — restores the terminal unconditionally when dropped.
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show);
    }
}

/// State for the inline line editor.
struct EditorState {
    buffer: String,
    completions: Vec<&'static str>,
    completion_index: usize,
    prev_comp_count: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    temp_buffer: String,
}

impl EditorState {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            completions: vec![],
            completion_index: 0,
            prev_comp_count: 0,
            history: vec![],
            history_index: None,
            temp_buffer: String::new(),
        }
    }

    fn update_completions(&mut self) {
        if self.buffer.starts_with('/') {
            self.completions = COMMANDS
                .iter()
                .filter(|(cmd, _)| cmd.starts_with(self.buffer.as_str()))
                .map(|(cmd, _)| *cmd)
                .collect();
        } else {
            self.completions.clear();
            self.completion_index = 0;
        }
    }

    fn accept_completion(&mut self) {
        if let Some(&cmd) = self.completions.get(self.completion_index) {
            self.buffer = cmd.to_string();
            self.completions.clear();
            self.completion_index = 0;
        }
    }

    fn next_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = (self.completion_index + 1) % self.completions.len();
        }
    }

    fn prev_completion(&mut self) {
        if !self.completions.is_empty() {
            self.completion_index = self
                .completion_index
                .checked_sub(1)
                .unwrap_or(self.completions.len() - 1);
        }
    }

    fn add_to_history(&mut self, line: String) {
        if !line.is_empty() && (self.history.is_empty() || self.history.last() != Some(&line)) {
            self.history.push(line);
        }
        self.history_index = None;
        self.temp_buffer.clear();
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.temp_buffer = self.buffer.clone();
            self.history_index = Some(self.history.len() - 1);
        } else if let Some(idx) = self.history_index {
            if idx > 0 {
                self.history_index = Some(idx - 1);
            }
        }
        if let Some(idx) = self.history_index {
            self.buffer = self.history[idx].clone();
        }
    }

    fn history_next(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                self.history_index = Some(idx + 1);
                self.buffer = self.history[idx + 1].clone();
            } else {
                self.history_index = None;
                self.buffer = self.temp_buffer.clone();
            }
        }
    }
}

/// Print a line with \r\n for raw mode.
fn rln(line: &str) {
    print!("{}\r\n", line);
}

/// Flush stdout.
fn flush() {
    let _ = io::stdout().flush();
}

/// Redraw the prompt + current buffer + completions in place.
/// Clears previous completion lines before drawing new ones.
fn redraw(state: &EditorState, stdout: &mut impl Write) {
    // Move up to clear previous completions.
    if state.prev_comp_count > 0 {
        let _ = queue!(stdout, cursor::MoveUp(state.prev_comp_count as u16));
    }
    // Always anchor to start of line before clearing, or prompts will stack.
    let _ = queue!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::FromCursorDown)
    );

    // Prompt + buffer.
    let _ = queue!(
        stdout,
        SetAttribute(Attribute::Bold),
        SetForegroundColor(Color::Cyan),
        Print("sena"),
        ResetColor,
        SetAttribute(Attribute::Dim),
        Print(" \u{203A} "),
        ResetColor,
        Print(&state.buffer),
    );

    // Completions below the prompt (if buffer starts with /).
    if !state.completions.is_empty() {
        for (i, cmd) in state.completions.iter().enumerate() {
            let desc = COMMANDS
                .iter()
                .find(|(c, _)| c == cmd)
                .map(|(_, d)| *d)
                .unwrap_or("");

            let marker = if i == state.completion_index {
                "▸"
            } else {
                " "
            };

            let _ = queue!(
                stdout,
                Print("\r\n"),
                SetAttribute(Attribute::Dim),
                Print(format!("  {} {:<16} {}", marker, cmd, desc)),
                ResetColor,
            );
        }

        // Move cursor back up to prompt line.
        let _ = queue!(
            stdout,
            cursor::MoveUp(state.completions.len() as u16),
            cursor::MoveToColumn((PROMPT_PREFIX_LEN + state.buffer.len()) as u16),
        );
    }

    let _ = stdout.flush();
}

/// Print output above the prompt (for async messages arriving mid-input).
/// Moves to a new line, prints the message, then redraws the prompt.
fn print_above(msg: &str, state: &mut EditorState, stdout: &mut impl Write) {
    // Clear the current prompt line.
    let _ = queue!(stdout, Print("\r"), terminal::Clear(ClearType::CurrentLine));
    // Move up over completions if any.
    for _ in 0..state.prev_comp_count {
        let _ = queue!(
            stdout,
            cursor::MoveUp(1),
            terminal::Clear(ClearType::CurrentLine),
        );
    }
    state.prev_comp_count = 0;

    // Print the message.
    let _ = queue!(stdout, Print(msg), Print("\r\n"));
    let _ = stdout.flush();

    // Redraw prompt.
    redraw(state, stdout);
}

/// Run the interactive shell. Returns the exit reason for the restart loop.
pub async fn run(runtime: Runtime) -> Result<ShellExitReason> {
    // ── Enter raw mode (no alternate screen — preserve scrollback) ───────────
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    let _guard = TerminalGuard; // restores terminal on drop

    // Print the help screen (using raw-mode-aware line endings).
    print_banner_raw();
    print_help_raw();

    // ── Ctrl-C shutdown watch ─────────────────────────────────────────────────
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    // ── Keyboard event reader (spawned blocking) ─────────────────────────────
    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel::<event::Event>();
    let key_tx2 = key_tx.clone();
    tokio::task::spawn_blocking(move || loop {
        if event::poll(Duration::from_millis(200)).unwrap_or(false) {
            if let Ok(ev) = event::read() {
                if key_tx2.send(ev).is_err() {
                    break;
                }
            }
        }
        if key_tx2.is_closed() {
            break;
        }
    });

    // ── Bus subscriber for verbose mode and inference responses ───────────────
    let mut bus_rx = runtime.bus.subscribe_broadcast();

    // ── Shell state ───────────────────────────────────────────────────────────
    let mut editor = EditorState::new();
    let mut verbose = false;
    let mut pending_inference_id: Option<u64> = None;
    let mut exit_reason = ShellExitReason::Quit;

    // Show initial prompt.
    redraw(&editor, &mut stdout);
    flush();

    // ── Main REPL loop ────────────────────────────────────────────────────────
    loop {
        tokio::select! {
            biased;

            // Ctrl-C
            _ = shutdown_rx.changed() => {
                rln("");
                break;
            }

            // Bus events (verbose mode + inference responses)
            bcast = bus_rx.recv() => {
                if let Ok(ev) = bcast {
                    match &ev {
                        // Inference response for a pending chat
                        Event::Inference(InferenceEvent::InferenceCompleted { text, request_id, .. })
                            if pending_inference_id == Some(*request_id) =>
                        {
                            pending_inference_id = None;
                            if text.trim().is_empty() {
                                let msg = format!("  {}{}✗{}  model returned empty response", BOLD, RED, RESET);
                                print_above(&msg, &mut editor, &mut stdout);
                            } else {
                                let formatted = format_chat_response(text);
                                print_above(&formatted, &mut editor, &mut stdout);
                            }
                        }
                        Event::Inference(InferenceEvent::InferenceFailed { request_id, reason })
                            if pending_inference_id == Some(*request_id) =>
                        {
                            pending_inference_id = None;
                            let msg = format!("  {}{}✗{}  model error: {}", BOLD, RED, RESET, reason);
                            print_above(&msg, &mut editor, &mut stdout);
                        }
                        Event::Inference(InferenceEvent::ModelLoaded { name, backend }) => {
                            if verbose || pending_inference_id.is_some() {
                                let msg = format!("  {}·{}  model loaded: {} {}({}){}", DIM, RESET, name, DIM, backend, RESET);
                                print_above(&msg, &mut editor, &mut stdout);
                            }
                        }
                        Event::Inference(InferenceEvent::BackendMismatchWarning { detected, compiled }) => {
                            let msg = format!(
                                "  {}{}⚠{}  backend mismatch: {} detected, but llama-cpp-2 compiled as {}",
                                BOLD, YELLOW, RESET, detected, compiled
                            );
                            print_above(&msg, &mut editor, &mut stdout);
                        }
                        _ if verbose => {
                            let msg = verbose_format(&ev);
                            if let Some(m) = msg {
                                print_above(&m, &mut editor, &mut stdout);
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Keyboard events
            Some(ev) = key_rx.recv() => {
                match ev {
                    event::Event::Key(KeyEvent { code, modifiers, kind, .. }) => {
                        // Filter out key release events to prevent duplicate input on Windows.
                        if kind != KeyEventKind::Press {
                            continue;
                        }
                        match (code, modifiers) {

                            // Quit
                            (KeyCode::Char('c'), KeyModifiers::CONTROL) |
                            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                rln("");
                                break;
                            }

                            // Submit line
                            (KeyCode::Enter, _) => {
                                let line = editor.buffer.trim().to_string();
                                editor.buffer.clear();
                                editor.completions.clear();
                                editor.prev_comp_count = 0;

                                // Clear the prompt line and move to new line.
                                let _ = queue!(stdout,
                                    Print("\r"),
                                    terminal::Clear(ClearType::CurrentLine),
                                    Print(format!("{}sena{} {}›{} {}", BOLD, RESET, DIM, RESET, &line)),
                                    Print("\r\n"),
                                );
                                let _ = stdout.flush();

                                if line.is_empty() {
                                    redraw(&editor, &mut stdout);
                                    continue;
                                }

                                editor.add_to_history(line.clone());

                                // Dispatch
                                let result = dispatch_line(
                                    &line,
                                    &runtime,
                                    &mut bus_rx,
                                    &mut verbose,
                                    &mut stdout,
                                    &mut pending_inference_id,
                                ).await;

                                match result {
                                    DispatchResult::Continue => {}
                                    DispatchResult::Quit => { exit_reason = ShellExitReason::Quit; break; }
                                    DispatchResult::Restart => { exit_reason = ShellExitReason::Restart; break; }
                                }

                                editor.prev_comp_count = 0;
                                redraw(&editor, &mut stdout);
                            }

                            // Backspace
                            (KeyCode::Backspace, _) => {
                                editor.buffer.pop();
                                editor.update_completions();
                                editor.completion_index = 0;
                                editor.prev_comp_count = editor.completions.len();
                                redraw(&editor, &mut stdout);
                            }

                            // Tab — accept first/current completion
                            (KeyCode::Tab, _) => {
                                editor.accept_completion();
                                editor.update_completions();
                                editor.prev_comp_count = editor.completions.len();
                                redraw(&editor, &mut stdout);
                            }

                            // Arrow down — next completion or next history
                            (KeyCode::Down, _) => {
                                if !editor.completions.is_empty() {
                                    editor.next_completion();
                                } else {
                                    editor.history_next();
                                    editor.update_completions();
                                    editor.completion_index = 0;
                                    editor.prev_comp_count = editor.completions.len();
                                }
                                redraw(&editor, &mut stdout);
                            }

                            // Arrow up — prev completion or prev history
                            (KeyCode::Up, _) => {
                                if !editor.completions.is_empty() {
                                    editor.prev_completion();
                                } else {
                                    editor.history_prev();
                                    editor.update_completions();
                                    editor.completion_index = 0;
                                    editor.prev_comp_count = editor.completions.len();
                                }
                                redraw(&editor, &mut stdout);
                            }

                            // Escape — clear completions / clear line
                            (KeyCode::Esc, _) => {
                                if !editor.completions.is_empty() {
                                    editor.completions.clear();
                                } else {
                                    editor.buffer.clear();
                                }
                                editor.completion_index = 0;
                                editor.prev_comp_count = 0;
                                redraw(&editor, &mut stdout);
                            }

                            // Regular character
                            (KeyCode::Char(c), mods) if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) => {
                                editor.buffer.push(c);
                                editor.update_completions();
                                editor.completion_index = 0;
                                editor.prev_comp_count = editor.completions.len();
                                redraw(&editor, &mut stdout);
                            }

                            _ => {}
                        }
                    }
                    event::Event::Resize(_, _) => {
                        redraw(&editor, &mut stdout);
                    }
                    _ => {}
                }
            }
        }
    }

    // ── Graceful shutdown ─────────────────────────────────────────────────────
    drop(_guard); // leaves alternate screen
    println!();
    display::info("Shutting down actors...");
    let timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    runtime::shutdown(runtime, timeout).await?;
    display::success("Sena stopped cleanly.");

    Ok(exit_reason)
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

enum DispatchResult {
    Continue,
    Quit,
    Restart,
}

async fn dispatch_line(
    line: &str,
    runtime: &Runtime,
    bus_rx: &mut broadcast::Receiver<Event>,
    verbose: &mut bool,
    stdout: &mut impl Write,
    pending_inference_id: &mut Option<u64>,
) -> DispatchResult {
    let lower = line.to_lowercase();
    let cmd = lower.split_whitespace().next().unwrap_or("");

    match cmd {
        "/observation" | "/obs" => {
            run_query(TransparencyQuery::CurrentObservation, runtime, stdout).await;
        }
        "/memory" | "/mem" => {
            run_query(TransparencyQuery::UserMemory, runtime, stdout).await;
        }
        "/explanation" | "/why" => {
            run_query(TransparencyQuery::InferenceExplanation, runtime, stdout).await;
        }
        "/models" => {
            if let Some(restart) = run_models(runtime, stdout, bus_rx).await {
                return restart;
            }
        }
        "/verbose" => {
            *verbose = !*verbose;
            let state = if *verbose { "ON" } else { "OFF" };
            rln(&format!(
                "  {}·{}  Verbose logging: {}{}{}",
                DIM, RESET, BOLD, state, RESET
            ));
        }
        "/help" | "/h" => {
            print_help_raw();
        }
        "/quit" | "/exit" | "/q" => {
            return DispatchResult::Quit;
        }
        _ if line.starts_with('/') => {
            rln(&format!(
                "  {}{}✗{}  unknown command '{}'. Type /help for commands.",
                BOLD, RED, RESET, line
            ));
        }
        // Free text → inference chat
        _ => {
            send_chat(line, runtime, stdout, pending_inference_id).await;
        }
    }

    DispatchResult::Continue
}

// ── Query helpers ─────────────────────────────────────────────────────────────

async fn run_query(q: TransparencyQuery, runtime: &Runtime, _stdout: &mut impl Write) {
    let label = match &q {
        TransparencyQuery::CurrentObservation => "Current Observation",
        TransparencyQuery::UserMemory => "Memory",
        TransparencyQuery::InferenceExplanation => "Last Inference",
    };

    rln(&format!("  {}·{}  Querying...", DIM, RESET));

    match query::query_on_bus(q, &runtime.bus).await {
        Ok(output) => {
            rln(&format!("\r\n  {}{}━━  {}{}", BOLD, CYAN, label, RESET));
            rln("");
            for line in output.lines() {
                rln(&format!("  {}", line));
            }
            rln("");
        }
        Err(e) => {
            rln(&format!("  {}{}✗{}  {}", BOLD, RED, RESET, e));
        }
    }
}

// ── Models helper ─────────────────────────────────────────────────────────────

async fn run_models(
    runtime: &Runtime,
    stdout: &mut impl Write,
    _bus_rx: &mut broadcast::Receiver<Event>,
) -> Option<DispatchResult> {
    let models = match model_selector::discover_and_print_menu(runtime).await {
        Ok(m) => m,
        Err(e) => {
            rln(&format!("  {}{}✗{}  {}", BOLD, RED, RESET, e));
            return None;
        }
    };

    // Prompt for selection (blocking-style using crossterm in raw mode).
    let _ = queue!(
        stdout,
        Print(format!(
            "  {}>{} Enter number or model name (Enter to keep current): ",
            CYAN, RESET
        )),
    );
    let _ = stdout.flush();

    let selection = read_line_raw().await;
    let trimmed = selection.trim().to_string();

    if trimmed.is_empty() {
        rln(&format!("  {}·{}  No change made.", DIM, RESET));
        return None;
    }

    match model_selector::apply_selection(&trimmed, &models, runtime).await {
        Ok(name) => {
            rln("");
            rln(&format!(
                "  {}{}✓{}  Selected: {}",
                BOLD, GREEN, RESET, name
            ));
            rln(&format!("  {}·{}  Saved to config.", DIM, RESET));

            // Ask whether to restart now.
            let _ = queue!(
                stdout,
                Print(format!(
                    "  {}>{} Restart now to apply? (y/N): ",
                    CYAN, RESET
                )),
            );
            let _ = stdout.flush();

            let answer = read_line_raw().await;
            let answer = answer.trim().to_lowercase();
            rln("");

            if answer == "y" || answer == "yes" {
                return Some(DispatchResult::Restart);
            } else {
                rln(&format!(
                    "  {}·{}  Restart Sena manually to use the new model.",
                    DIM, RESET
                ));
            }
        }
        Err(e) => {
            rln(&format!("  {}{}✗{}  {}", BOLD, RED, RESET, e));
        }
    }

    None
}

/// Read a line in raw mode (blocking via spawn_blocking).
async fn read_line_raw() -> String {
    tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        loop {
            if event::poll(Duration::from_secs(120)).unwrap_or(false) {
                if let Ok(event::Event::Key(k)) = event::read() {
                    // Filter out key release events to prevent duplicate input.
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    match k.code {
                        KeyCode::Enter => break,
                        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                            buf.push(c);
                            print!("{}", c);
                            let _ = io::stdout().flush();
                        }
                        KeyCode::Backspace => {
                            if buf.pop().is_some() {
                                print!("\x08 \x08");
                                let _ = io::stdout().flush();
                            }
                        }
                        KeyCode::Esc => {
                            buf.clear();
                            break;
                        }
                        _ => {}
                    }
                }
            } else {
                break; // timeout
            }
        }
        buf
    })
    .await
    .unwrap_or_default()
}

// ── Chat ──────────────────────────────────────────────────────────────────────

async fn send_chat(
    prompt: &str,
    runtime: &Runtime,
    stdout: &mut impl Write,
    pending_id: &mut Option<u64>,
) {
    // Generate request id.
    let request_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);

    let _ = queue!(
        stdout,
        Print(format!(
            "  {}·{}  Thinking... (model response may take a moment)\r\n",
            DIM, RESET
        )),
    );
    let _ = stdout.flush();

    match runtime
        .bus
        .send_directed(
            "inference",
            Event::Inference(bus::events::inference::InferenceEvent::InferenceRequested {
                prompt: prompt.to_owned(),
                priority: Priority::High,
                request_id,
            }),
        )
        .await
    {
        Ok(()) => {
            *pending_id = Some(request_id);
        }
        Err(e) => {
            rln(&format!(
                "  {}{}✗{}  could not reach inference actor: {}",
                BOLD, RED, RESET, e
            ));
        }
    }
}

fn format_chat_response(text: &str) -> String {
    let mut out = format!("\r\n  {}{}━━  Response{}\r\n\r\n", BOLD, CYAN, RESET);
    for line in text.lines() {
        out.push_str(&format!("  {}\r\n", line));
    }
    out.push_str("\r\n");
    out
}

// ── Verbose formatting ────────────────────────────────────────────────────────

fn verbose_format(ev: &Event) -> Option<String> {
    match ev {
        Event::CTP(bus::events::CTPEvent::ThoughtEventTriggered(_)) => Some(format!(
            "  {}[verbose] CTP: thought triggered{}",
            DIM, RESET
        )),
        Event::Soul(bus::events::SoulEvent::EventLogged(e)) => Some(format!(
            "  {}[verbose] Soul: event logged (row {}){}",
            DIM, e.row_id, RESET
        )),
        Event::Platform(bus::events::PlatformEvent::WindowChanged(w)) => Some(format!(
            "  {}[verbose] Window: {}{}",
            DIM, w.app_name, RESET
        )),
        Event::Inference(InferenceEvent::ModelLoaded { name, .. }) => Some(format!(
            "  {}[verbose] Inference: model loaded — {}{}",
            DIM, name, RESET
        )),
        _ => None,
    }
}

// ── Banner / help (raw-mode aware) ────────────────────────────────────────────

fn print_banner_raw() {
    rln(&format!("{}{}  ", BOLD, CYAN));
    rln("  \u{256F}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}");
    rln("  \u{2551}       \u{00B7} S E N A \u{00B7}                \u{2551}");
    rln("  \u{2551}       local-first ambient AI     \u{2551}");
    rln("  \u{255A}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255D}");
    rln(RESET);
}

fn print_help_raw() {
    rln(&format!("\r\n  {}{}━━  Commands{}", BOLD, CYAN, RESET));
    rln("");
    rln(&format!(
        "  {}/observation{}  or /obs   What are you observing right now?",
        BOLD, RESET
    ));
    rln(&format!(
        "  {}/memory{}       or /mem   What do you remember about me?",
        BOLD, RESET
    ));
    rln(&format!(
        "  {}/explanation{}  or /why   Why did you say that?",
        BOLD, RESET
    ));
    rln(&format!(
        "  {}/models{}               Select which Ollama model to use",
        BOLD, RESET
    ));
    rln(&format!(
        "  {}/verbose{}              Toggle verbose actor-event logging",
        BOLD, RESET
    ));
    rln(&format!(
        "  {}/help{}                 Show this message",
        BOLD, RESET
    ));
    rln(&format!(
        "  {}/quit{}                 Exit Sena",
        BOLD, RESET
    ));
    rln("");
    rln(&format!(
        "  {}·{}  Type any message to chat with the model.",
        DIM, RESET
    ));
    rln(&format!(
        "  {}·{}  Tab / Down / Up to navigate / completions.",
        DIM, RESET
    ));
    rln("");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_state_history_tracks_commands() {
        let mut state = EditorState::new();
        assert!(state.history.is_empty());

        state.add_to_history("hello".to_string());
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0], "hello");

        state.add_to_history("world".to_string());
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.history[1], "world");
    }

    #[test]
    fn editor_state_history_ignores_duplicates() {
        let mut state = EditorState::new();
        state.add_to_history("same".to_string());
        state.add_to_history("same".to_string());
        assert_eq!(state.history.len(), 1);
    }

    #[test]
    fn editor_state_history_ignores_empty() {
        let mut state = EditorState::new();
        state.add_to_history("".to_string());
        assert!(state.history.is_empty());
    }

    #[test]
    fn editor_state_history_prev_navigates_backward() {
        let mut state = EditorState::new();
        state.add_to_history("first".to_string());
        state.add_to_history("second".to_string());
        state.add_to_history("third".to_string());

        state.buffer = "current".to_string();
        state.history_prev();
        assert_eq!(state.buffer, "third");
        assert_eq!(state.temp_buffer, "current");

        state.history_prev();
        assert_eq!(state.buffer, "second");

        state.history_prev();
        assert_eq!(state.buffer, "first");

        // Should stay at first
        state.history_prev();
        assert_eq!(state.buffer, "first");
    }

    #[test]
    fn editor_state_history_next_navigates_forward() {
        let mut state = EditorState::new();
        state.add_to_history("first".to_string());
        state.add_to_history("second".to_string());

        state.buffer = "current".to_string();
        state.history_prev(); // -> "second"
        state.history_prev(); // -> "first"

        state.history_next(); // -> "second"
        assert_eq!(state.buffer, "second");

        state.history_next(); // -> "current" (temp buffer)
        assert_eq!(state.buffer, "current");
    }

    #[test]
    fn editor_state_history_empty_returns_early() {
        let mut state = EditorState::new();
        state.buffer = "test".to_string();
        state.history_prev();
        assert_eq!(state.buffer, "test"); // unchanged
    }

    #[test]
    fn format_chat_response_handles_multiline() {
        let text = "line one\nline two\nline three";
        let result = format_chat_response(text);
        assert!(result.contains("line one"));
        assert!(result.contains("line two"));
        assert!(result.contains("line three"));
        assert!(result.contains("━━  Response"));
    }

    #[test]
    fn format_chat_response_preserves_empty_lines() {
        let text = "first\n\nlast";
        let result = format_chat_response(text);
        assert!(result.contains("first"));
        assert!(result.contains("last"));
    }
}
