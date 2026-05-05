use crate::config_editor::ConfigEditor;
use crate::error::CliError;
use crate::theme;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ipc::IpcClient;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::info;

#[derive(Clone, Debug)]
struct LoopInfo {
    name: String,
    description: String,
    enabled: bool,
}

#[derive(Clone, Copy, Debug)]
struct VramState {
    used_mb: u32,
    total_mb: u32,
    percent: u8,
    updated_at: Instant,
}

const HELP_CORE: &[(&str, &str, &str)] = &[
    ("/help, /?", "show the command guide", "/help"),
    (
        "/status, /health",
        "show daemon and actor status",
        "/status",
    ),
    ("/quit, /exit, /bye", "close the CLI", "/quit"),
];

const HELP_SPEECH: &[(&str, &str, &str)] = &[
    (
        "/listen, /mic",
        "start live microphone transcription",
        "/listen",
    ),
    (
        "/stop, /end",
        "stop listening and finalize the transcript",
        "/stop",
    ),
    ("/speech, /audio", "show speech subsystem status", "/speech"),
];

const HELP_MODELS: &[(&str, &str, &str)] = &[
    ("/models", "list available local models", "/models"),
    (
        "/model load <path>",
        "load a model from disk",
        "/model load C:/models/qwen.gguf",
    ),
    (
        "/inference, /infer",
        "show inference subsystem status",
        "/inference",
    ),
];

const HELP_MEMORY: &[(&str, &str, &str)] = &[
    (
        "/observation, /obs",
        "show Sena's current observation snapshot",
        "/observation",
    ),
    (
        "/memory, /mem",
        "show what Sena remembers about you",
        "/memory",
    ),
    (
        "/memory-stats, /memstats",
        "show memory store stats",
        "/memory-stats",
    ),
    (
        "/explanation, /explain <thought_id>",
        "explain a specific thought",
        "/explanation latest",
    ),
    (
        "/query, /search <text>",
        "search memory",
        "/query project roadmap",
    ),
    ("/config, /settings", "open the config editor", "/config"),
];

const HELP_RUNTIME: &[(&str, &str, &str)] = &[
    ("/loops, /loop", "list background loops", "/loops"),
    (
        "/loops <name> on|off",
        "toggle a specific loop",
        "/loops speech off",
    ),
    ("/events, /watch", "subscribe to daemon events", "/events"),
    ("/shutdown", "stop the daemon", "/shutdown"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandCategory {
    Core,
    Speech,
    Models,
    Memory,
    Runtime,
}

impl CommandCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Core => "Core",
            Self::Speech => "Speech",
            Self::Models => "Models",
            Self::Memory => "Memory",
            Self::Runtime => "Runtime",
        }
    }
}

struct SlashCommand {
    command: &'static str,
    description: &'static str,
    category: CommandCategory,
}

const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        command: "/help",
        description: "Show the command guide",
        category: CommandCategory::Core,
    },
    SlashCommand {
        command: "/status",
        description: "Show daemon and actor status",
        category: CommandCategory::Core,
    },
    SlashCommand {
        command: "/quit",
        description: "Close the CLI",
        category: CommandCategory::Core,
    },
    SlashCommand {
        command: "/listen",
        description: "Start live transcription",
        category: CommandCategory::Speech,
    },
    SlashCommand {
        command: "/stop",
        description: "Stop listening",
        category: CommandCategory::Speech,
    },
    SlashCommand {
        command: "/speech",
        description: "Show speech status",
        category: CommandCategory::Speech,
    },
    SlashCommand {
        command: "/models",
        description: "Open the model picker",
        category: CommandCategory::Models,
    },
    SlashCommand {
        command: "/model load",
        description: "Load a model by path",
        category: CommandCategory::Models,
    },
    SlashCommand {
        command: "/inference",
        description: "Show inference status",
        category: CommandCategory::Models,
    },
    SlashCommand {
        command: "/observation",
        description: "Show current observation snapshot",
        category: CommandCategory::Memory,
    },
    SlashCommand {
        command: "/memory",
        description: "Show remembered user context",
        category: CommandCategory::Memory,
    },
    SlashCommand {
        command: "/memory-stats",
        description: "Show memory stats",
        category: CommandCategory::Memory,
    },
    SlashCommand {
        command: "/explanation",
        description: "Explain a thought by id",
        category: CommandCategory::Memory,
    },
    SlashCommand {
        command: "/query",
        description: "Search memory",
        category: CommandCategory::Memory,
    },
    SlashCommand {
        command: "/config",
        description: "Open config editor",
        category: CommandCategory::Memory,
    },
    SlashCommand {
        command: "/loops",
        description: "List background loops",
        category: CommandCategory::Runtime,
    },
    SlashCommand {
        command: "/events",
        description: "Subscribe to daemon events",
        category: CommandCategory::Runtime,
    },
    SlashCommand {
        command: "/shutdown",
        description: "Shut down the daemon",
        category: CommandCategory::Runtime,
    },
];

#[derive(Clone, Debug)]
struct SlashDropdown {
    filtered: Vec<usize>,
    selected: usize,
    no_matches: bool,
}

impl SlashDropdown {
    fn from_prefix(prefix: &str) -> Self {
        let filtered = SLASH_COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, command)| command.command.starts_with(prefix))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let no_matches = filtered.is_empty() && !prefix.is_empty() && prefix != "/";
        Self {
            filtered,
            selected: 0,
            no_matches,
        }
    }

    fn update(&mut self, prefix: &str) {
        *self = Self::from_prefix(prefix);
    }

    fn next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    fn prev(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.filtered.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    fn selected_command(&self) -> Option<&'static str> {
        self.filtered
            .get(self.selected)
            .and_then(|&index| SLASH_COMMANDS.get(index))
            .map(|command| command.command)
    }

    fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }
}

#[derive(Clone, Debug)]
struct ModelChoice {
    name: String,
    path: String,
    size_bytes: u64,
}

#[derive(Clone, Debug)]
struct ModelModal {
    models: Vec<ModelChoice>,
    selected: usize,
}

impl ModelModal {
    fn new(models: Vec<ModelChoice>) -> Self {
        Self {
            models,
            selected: 0,
        }
    }

    fn next(&mut self) {
        if !self.models.is_empty() {
            self.selected = (self.selected + 1) % self.models.len();
        }
    }

    fn prev(&mut self) {
        if self.models.is_empty() {
            return;
        }
        if self.selected == 0 {
            self.selected = self.models.len() - 1;
        } else {
            self.selected -= 1;
        }
    }

    fn selected(&self) -> Option<&ModelChoice> {
        self.models.get(self.selected)
    }
}

#[derive(Clone, Debug)]
enum ModalState {
    Models(ModelModal),
}

struct ShellRenderState<'a> {
    message_log: &'a [String],
    loops: &'a [LoopInfo],
    input_buffer: &'a str,
    daemon_status: &'a str,
    daemon_uptime_secs: u64,
    log_scroll: usize,
    vram: Option<VramState>,
    slash_dropdown: Option<&'a SlashDropdown>,
    modal: Option<&'a ModalState>,
}

pub struct Shell {
    ipc: IpcClient,
    message_log: Arc<Mutex<Vec<String>>>,
    loops: Arc<Mutex<HashMap<String, LoopInfo>>>,
    vram: Arc<Mutex<Option<VramState>>>,
    input_buffer: String,
    should_quit: bool,
    daemon_status: String,
    daemon_uptime_secs: u64,
    daemon_uptime_anchor: Instant,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    connection_alive: Arc<AtomicBool>,
    log_scroll: usize,
    quit_armed: bool,
    slash_dropdown: Option<SlashDropdown>,
    modal: Option<ModalState>,
}

impl Shell {
    pub async fn new(mut ipc: IpcClient) -> Result<Self, CliError> {
        enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let backend = CrosstermBackend::new(stdout);
        let terminal =
            Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        let message_log = Arc::new(Mutex::new(vec![
            "Welcome to Sena CLI".to_string(),
            "Type /help for commands".to_string(),
        ]));
        let loops: Arc<Mutex<HashMap<String, LoopInfo>>> = Arc::new(Mutex::new(HashMap::new()));
        let vram: Arc<Mutex<Option<VramState>>> = Arc::new(Mutex::new(None));

        let mut daemon_uptime_secs = 0;

        match ipc.send("events.subscribe", json!({})).await {
            Ok(_) => {
                if let Ok(mut log) = message_log.lock() {
                    log.push("[SYS] subscribed to daemon event stream".to_string());
                }
            }
            Err(e) => {
                if let Ok(mut log) = message_log.lock() {
                    log.push(format!("[ERR] events.subscribe failed: {}", e));
                }
            }
        }

        match ipc.send("runtime.ping", json!({})).await {
            Ok(response) => {
                daemon_uptime_secs = response
                    .get("uptime_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
            }
            Err(e) => {
                if let Ok(mut log) = message_log.lock() {
                    log.push(format!("[ERR] runtime.ping failed: {}", e));
                }
            }
        }

        if let Ok(response) = ipc.send("loops.list", json!({})).await
            && let Some(loops_array) = response.get("loops").and_then(|v| v.as_array())
            && let Ok(mut loops_map) = loops.lock()
        {
            for loop_data in loops_array {
                let name = loop_data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let description = loop_data
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let enabled = loop_data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                loops_map.insert(
                    name.clone(),
                    LoopInfo {
                        name,
                        description,
                        enabled,
                    },
                );
            }
        }

        let push_log = Arc::clone(&message_log);
        let push_loops = Arc::clone(&loops);
        let push_vram = Arc::clone(&vram);
        let connection_alive = Arc::new(AtomicBool::new(true));
        let connection_alive_task = Arc::clone(&connection_alive);
        let mut push_rx = ipc.subscribe_events();

        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let data = event.get("data").cloned().unwrap_or(Value::Null);

                if event_type == "LoopStatusChanged"
                    && let (Some(loop_name), Some(enabled)) = (
                        data.get("loop_name").and_then(|v| v.as_str()),
                        data.get("enabled").and_then(|v| v.as_bool()),
                    )
                    && let Ok(mut loops_map) = push_loops.lock()
                    && let Some(loop_info) = loops_map.get_mut(loop_name)
                {
                    loop_info.enabled = enabled;
                }

                if event_type == "VramUsageUpdated" {
                    let used_mb = data.get("used_mb").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let total_mb =
                        data.get("total_mb").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let percent = data.get("percent").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                    if let Ok(mut vram_state) = push_vram.lock() {
                        *vram_state = Some(VramState {
                            used_mb,
                            total_mb,
                            percent,
                            updated_at: Instant::now(),
                        });
                    }
                    continue;
                }

                if let Some(line) = Self::format_push_event(&event)
                    && let Ok(mut log) = push_log.lock()
                {
                    Self::append_push_line(&mut log, line);
                }
            }
            connection_alive_task.store(false, Ordering::SeqCst);
        });

        Ok(Self {
            ipc,
            message_log,
            loops,
            vram,
            input_buffer: String::new(),
            should_quit: false,
            daemon_status: "Connected".to_string(),
            daemon_uptime_secs,
            daemon_uptime_anchor: Instant::now(),
            terminal,
            connection_alive,
            log_scroll: 0,
            quit_armed: false,
            slash_dropdown: None,
            modal: None,
        })
    }

    fn append_push_line(log: &mut Vec<String>, line: String) {
        if let Some(fragment) = line.strip_prefix("[STT~] ") {
            if let Some(last) = log.last_mut()
                && last.starts_with("[STT~] ")
            {
                *last = format!("[STT~] {}", fragment);
            } else {
                log.push(line);
            }
        } else if let Some(finalized) = line.strip_prefix("[STT!] ") {
            if let Some(last) = log.last_mut()
                && last.starts_with("[STT~] ")
            {
                *last = format!("[STT] \"{}\"", finalized);
            } else {
                log.push(format!("[STT] \"{}\"", finalized));
            }
        } else {
            log.push(line);
        }

        if log.len() > 500 {
            log.drain(0..100);
        }
    }

    fn format_push_event(event: &Value) -> Option<String> {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let data = event.get("data").cloned().unwrap_or(Value::Null);

        match event_type {
            "TranscriptionCompleted" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let conf = data.get("confidence").and_then(|v| v.as_f64());
                if let Some(confidence) = conf {
                    Some(format!("[STT] \"{}\" (conf: {:.2})", text, confidence))
                } else {
                    Some(format!("[STT] \"{}\"", text))
                }
            }
            "ListenModeTranscription" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                Some(format!("[STT~] {}", text))
            }
            "LowConfidenceTranscription" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let confidence = data
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                Some(format!("[unclear] \"{}\" (conf: {:.2})", text, confidence))
            }
            "WakewordDetected" => {
                let confidence = data
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                Some(format!("[wakeword] detected (conf: {:.2})", confidence))
            }
            "WakewordSuppressed" => {
                let reason = data
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("[wakeword] suppressed ({})", reason))
            }
            "WakewordResumed" => Some("[wakeword] resumed".to_string()),
            "ListenModeTranscriptFinalized" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                Some(format!("[STT!] {}", text))
            }
            "InferenceSentenceReady" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                Some(format!("[INF] {}", text))
            }
            "InferenceStreamCompleted" => {
                let token_count = data
                    .get("token_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                Some(format!("[INF] ✓ stream done ({} tokens)", token_count))
            }
            "InferenceCompleted" => {
                let token_count = data
                    .get("token_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let causal_id = data.get("causal_id").and_then(|v| v.as_u64()).unwrap_or(0);
                Some(format!(
                    "[INF] ✓ response complete ({} tokens, id {})",
                    token_count, causal_id
                ))
            }
            "SpeakingStarted" => Some("[TTS] speaking started".to_string()),
            "SpeakingCompleted" => Some("[TTS] done".to_string()),
            "MemoryWriteCompleted" => Some("[MEM] write ok".to_string()),
            "MemoryWriteFailed" => {
                let reason = data
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("[MEM] failed: {}", reason))
            }
            "ActorFailed" => {
                let actor = data.get("actor").and_then(|v| v.as_str()).unwrap_or("?");
                let reason = data
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("[ERR] {}: {}", actor, reason))
            }
            "ThoughtEventTriggered" => {
                let app = data.get("app").and_then(|v| v.as_str()).unwrap_or("?");
                let task = data
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("[CTP] app: {}, task: {}", app, task))
            }
            "BootComplete" => Some("[SYS] boot complete".to_string()),
            "ConfigUpdated" => Some("[SYS] config updated".to_string()),
            "PlatformWindowChanged" => {
                let app = data.get("app").and_then(|v| v.as_str()).unwrap_or("?");
                let title = data.get("title").and_then(|v| v.as_str()).unwrap_or("");
                if title.is_empty() {
                    Some(format!("[PLT] window: {}", app))
                } else {
                    Some(format!("[PLT] window: {} — {}", app, title))
                }
            }
            "PlatformClipboardChanged" => {
                let chars = data.get("char_count").and_then(|v| v.as_u64()).unwrap_or(0);
                Some(format!("[PLT] clipboard: {} chars", chars))
            }
            "PlatformFileEvent" => {
                let kind = data.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                Some(format!("[PLT] file: {} {}", kind, path))
            }
            "LoopStatusChanged" => {
                let loop_name = data
                    .get("loop_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Some(format!(
                    "[SYS] loop {} {}",
                    loop_name,
                    if enabled { "enabled" } else { "disabled" }
                ))
            }
            "VramUsageUpdated" => None,
            _ => Some(format!("[EVENT] {}", event)),
        }
    }

    pub async fn run(mut self) -> Result<(), CliError> {
        info!("Shell TUI starting");

        if let (Ok(log), Ok(loops_map)) = (self.message_log.lock(), self.loops.lock()) {
            let loops_vec: Vec<LoopInfo> = loops_map.values().cloned().collect();
            let uptime_secs = self.current_uptime_secs();
            Self::render_tui(
                &mut self.terminal,
                ShellRenderState {
                    message_log: &log,
                    loops: &loops_vec,
                    input_buffer: &self.input_buffer,
                    daemon_status: &self.daemon_status,
                    daemon_uptime_secs: uptime_secs,
                    log_scroll: self.log_scroll,
                    vram: self.vram.lock().ok().and_then(|v| *v),
                    slash_dropdown: self.slash_dropdown.as_ref(),
                    modal: self.modal.as_ref(),
                },
            )
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        }

        while !self.should_quit {
            if !self.connection_alive.load(Ordering::SeqCst) {
                self.log_message("Daemon disconnected. Exiting...".to_string());
                break;
            }

            if event::poll(Duration::from_millis(100))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && let Event::Key(key) =
                    event::read().map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key_event(key.code, key.modifiers).await?;
            }

            if let (Ok(log), Ok(loops_map)) = (self.message_log.lock(), self.loops.lock()) {
                let loops_vec: Vec<LoopInfo> = loops_map.values().cloned().collect();
                let uptime_secs = self.current_uptime_secs();
                Self::render_tui(
                    &mut self.terminal,
                    ShellRenderState {
                        message_log: &log,
                        loops: &loops_vec,
                        input_buffer: &self.input_buffer,
                        daemon_status: &self.daemon_status,
                        daemon_uptime_secs: uptime_secs,
                        log_scroll: self.log_scroll,
                        vram: self.vram.lock().ok().and_then(|v| *v),
                        slash_dropdown: self.slash_dropdown.as_ref(),
                        modal: self.modal.as_ref(),
                    },
                )
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
            }
        }

        self.cleanup_terminal()?;
        info!("Shell TUI stopped");
        Ok(())
    }

    async fn handle_key_event(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<(), CliError> {
        if self.modal.is_some() {
            return self.handle_modal_key_event(code).await;
        }

        if self
            .slash_dropdown
            .as_ref()
            .is_some_and(|dropdown| !dropdown.is_empty() || dropdown.no_matches)
        {
            match code {
                KeyCode::Up => {
                    if let Some(dropdown) = &mut self.slash_dropdown {
                        dropdown.prev();
                    }
                    return Ok(());
                }
                KeyCode::Down => {
                    if let Some(dropdown) = &mut self.slash_dropdown {
                        dropdown.next();
                    }
                    return Ok(());
                }
                KeyCode::Tab => {
                    if let Some(command) = self
                        .slash_dropdown
                        .as_ref()
                        .and_then(|dropdown| dropdown.selected_command())
                    {
                        self.input_buffer = command.to_string();
                    }
                    self.refresh_slash_dropdown();
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.slash_dropdown = None;
                    return Ok(());
                }
                _ => {}
            }
        }

        if !matches!(code, KeyCode::Char('q') if modifiers.is_empty()) {
            self.quit_armed = false;
        }

        match code {
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('q') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('q') if modifiers.is_empty() => {
                if self.input_buffer.starts_with('/') {
                    self.input_buffer.push('q');
                } else if self.input_buffer.is_empty() {
                    if self.quit_armed {
                        self.should_quit = true;
                    } else {
                        self.quit_armed = true;
                        self.log_message("Press q again to quit, or start a /command.".to_string());
                    }
                } else {
                    self.input_buffer.push('q');
                }
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
                self.refresh_slash_dropdown();
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
                self.refresh_slash_dropdown();
            }
            KeyCode::Enter => {
                let input = self.input_buffer.clone();
                self.input_buffer.clear();
                self.slash_dropdown = None;
                self.log_scroll = 0;
                self.handle_input(input).await?;
            }
            KeyCode::Up => {
                let max_scroll = self.message_log.lock().map(|l| l.len()).unwrap_or(0);
                self.log_scroll = (self.log_scroll + 1).min(max_scroll.saturating_sub(1));
            }
            KeyCode::Down => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_modal_key_event(&mut self, code: KeyCode) -> Result<(), CliError> {
        match (&mut self.modal, code) {
            (Some(ModalState::Models(modal)), KeyCode::Up) => modal.prev(),
            (Some(ModalState::Models(modal)), KeyCode::Down) => modal.next(),
            (Some(ModalState::Models(_)), KeyCode::Esc) => {
                self.modal = None;
                self.log_message("Model selection cancelled.".to_string());
            }
            (Some(ModalState::Models(_)), KeyCode::Enter) => {
                self.handle_model_modal_enter().await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_input(&mut self, input: String) -> Result<(), CliError> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }

        self.log_message(format!("> {}", input));

        if input.starts_with('/') {
            self.handle_slash_command(input).await?;
        } else {
            self.log_message("Voice is primary. Type /help for manual commands.".to_string());
        }

        Ok(())
    }

    fn refresh_slash_dropdown(&mut self) {
        let prefix = self.input_buffer.split_whitespace().next().unwrap_or("");
        if prefix.starts_with('/') {
            if let Some(dropdown) = &mut self.slash_dropdown {
                dropdown.update(prefix);
            } else {
                self.slash_dropdown = Some(SlashDropdown::from_prefix(prefix));
            }
        } else {
            self.slash_dropdown = None;
        }
    }

    fn sync_uptime(&mut self, uptime_secs: u64) {
        self.daemon_uptime_secs = uptime_secs;
        self.daemon_uptime_anchor = Instant::now();
    }

    fn current_uptime_secs(&self) -> u64 {
        self.daemon_uptime_secs + self.daemon_uptime_anchor.elapsed().as_secs()
    }

    async fn handle_slash_command(&mut self, input: &str) -> Result<(), CliError> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        match parts[0] {
            "/help" | "/?" => self.cmd_help().await?,
            "/quit" | "/exit" | "/bye" => {
                self.should_quit = true;
            }
            "/status" | "/health" => self.cmd_status().await?,
            "/ping" | "/uptime" => self.cmd_ping().await?,
            "/shutdown" => self.cmd_shutdown().await?,
            "/models" => self.cmd_open_model_modal().await?,
            "/model" => match parts.get(1).copied() {
                Some("load") => self.cmd_load_model(parts.get(2).copied()).await?,
                _ => self.cmd_open_model_modal().await?,
            },
            "/load" => self.cmd_load_model(parts.get(1).copied()).await?,
            "/listen" | "/mic" => self.cmd_listen_start().await?,
            "/stop" | "/end" => self.cmd_listen_stop().await?,
            "/observation" | "/obs" => self.cmd_observation().await?,
            "/memory" | "/mem" => self.cmd_transparency_memory().await?,
            "/memory-stats" | "/memstats" => self.cmd_memory_stats().await?,
            "/explanation" | "/explain" => self.cmd_explanation(&parts[1..]).await?,
            "/query" | "/search" | "/recall" => self.cmd_memory_query(&parts[1..]).await?,
            "/config" | "/settings" => self.open_config_editor().await?,
            "/events" | "/watch" => self.cmd_events_subscribe().await?,
            "/inference" | "/infer" => self.cmd_inference_status().await?,
            "/speech" | "/audio" => self.cmd_speech_status().await?,
            "/loops" | "/loop" => match parts.get(1).copied() {
                Some("set") => {
                    self.cmd_loops(parts.get(2).copied(), parts.get(3).copied())
                        .await?
                }
                _ => {
                    self.cmd_loops(parts.get(1).copied(), parts.get(2).copied())
                        .await?
                }
            },
            other => {
                self.log_message(format!(
                    "Unknown command: {}. Type /help for grouped examples.",
                    other
                ));
            }
        }

        Ok(())
    }

    async fn cmd_help(&mut self) -> Result<(), CliError> {
        self.log_message("Manual command guide:".to_string());
        self.log_help_section("Core", HELP_CORE);
        self.log_help_section("Speech", HELP_SPEECH);
        self.log_help_section("Models", HELP_MODELS);
        self.log_help_section("Transparency + Memory + Config", HELP_MEMORY);
        self.log_help_section("Runtime", HELP_RUNTIME);
        Ok(())
    }

    async fn cmd_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("runtime.status", json!({})).await {
            Ok(response) => {
                let status = response
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let uptime = response
                    .get("uptime_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                self.sync_uptime(uptime);
                self.log_message(format!("Daemon {}. Uptime: {}s.", status, uptime));

                if let Some(actors) = response.get("actors").and_then(|v| v.as_array()) {
                    for actor in actors {
                        let name = actor.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let status_text = actor
                            .get("status")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| {
                                actor
                                    .get("status")
                                    .map_or("?".to_string(), |v| v.to_string())
                            });
                        self.log_message(format!("  {} — {}", name, status_text));
                    }
                }
            }
            Err(e) => {
                self.log_message(format!("Status command failed: {}", e));
            }
        }
        Ok(())
    }

    async fn cmd_ping(&mut self) -> Result<(), CliError> {
        match self.ipc.send("runtime.ping", json!({})).await {
            Ok(response) => {
                if let Some(uptime) = response.get("uptime_seconds").and_then(|v| v.as_u64()) {
                    self.sync_uptime(uptime);
                    self.log_message(format!("Daemon reachable. Uptime: {}s.", uptime));
                } else {
                    self.log_message(format!("Daemon replied: {}", response));
                }
            }
            Err(e) => {
                self.log_message(format!("Could not reach the daemon: {}", e));
            }
        }
        Ok(())
    }

    async fn cmd_shutdown(&mut self) -> Result<(), CliError> {
        match self.ipc.send("runtime.shutdown", json!({})).await {
            Ok(_) => {
                self.log_message(
                    "Shutdown requested. The daemon will disconnect shortly.".to_string(),
                );
                self.daemon_status = "Shutting down...".to_string();
                tokio::time::sleep(Duration::from_millis(500)).await;
                self.should_quit = true;
            }
            Err(e) => {
                self.log_message(format!("Shutdown command failed: {}", e));
            }
        }
        Ok(())
    }

    async fn cmd_open_model_modal(&mut self) -> Result<(), CliError> {
        match self.ipc.send("inference.list_models", json!({})).await {
            Ok(response) => {
                let Some(models) = response.get("models").and_then(|value| value.as_array()) else {
                    self.log_message(
                        "Could not open model picker: malformed response.".to_string(),
                    );
                    return Ok(());
                };

                let model_choices = models
                    .iter()
                    .filter_map(|model| {
                        Some(ModelChoice {
                            name: model.get("name")?.as_str()?.to_string(),
                            path: model.get("path")?.as_str()?.to_string(),
                            size_bytes: model
                                .get("size_bytes")
                                .and_then(|value| value.as_u64())
                                .unwrap_or(0),
                        })
                    })
                    .collect::<Vec<_>>();

                if model_choices.is_empty() {
                    self.log_message("No local GGUF models were found.".to_string());
                } else {
                    self.modal = Some(ModalState::Models(ModelModal::new(model_choices)));
                }
            }
            Err(e) => self.log_message(format!("Could not list models: {}", e)),
        }
        Ok(())
    }

    async fn cmd_load_model(&mut self, path: Option<&str>) -> Result<(), CliError> {
        let Some(path) = path else {
            self.log_message("Usage: /model load <path>".to_string());
            return Ok(());
        };

        match self
            .ipc
            .send("inference.load_model", json!({"path": path}))
            .await
        {
            Ok(response) => self.log_message(format!("Model load requested: {}", response)),
            Err(e) => self.log_message(format!("Could not load that model: {}", e)),
        }
        Ok(())
    }

    async fn cmd_listen_start(&mut self) -> Result<(), CliError> {
        match self.ipc.send("speech.listen_start", json!({})).await {
            Ok(response) => self.log_message(format!("Listening started: {}", response)),
            Err(e) => self.log_message(format!("Could not start listening: {}", e)),
        }
        Ok(())
    }

    async fn cmd_listen_stop(&mut self) -> Result<(), CliError> {
        match self.ipc.send("speech.listen_stop", json!({})).await {
            Ok(response) => self.log_message(format!("Listening stopped: {}", response)),
            Err(e) => self.log_message(format!("Could not stop listening: {}", e)),
        }
        Ok(())
    }

    async fn cmd_observation(&mut self) -> Result<(), CliError> {
        self.cmd_transparency_query("Current observation", json!("CurrentObservation"))
            .await
    }

    async fn cmd_transparency_memory(&mut self) -> Result<(), CliError> {
        self.cmd_transparency_query("Remembered user context", json!("UserMemory"))
            .await
    }

    async fn cmd_memory_stats(&mut self) -> Result<(), CliError> {
        match self.ipc.send("memory.stats", json!({})).await {
            Ok(response) => self.log_message(format!("Memory snapshot: {}", response)),
            Err(e) => self.log_message(format!("Could not read memory stats: {}", e)),
        }
        Ok(())
    }

    async fn cmd_explanation(&mut self, args: &[&str]) -> Result<(), CliError> {
        let Some(thought_id) = args.first().copied() else {
            self.log_message(
                "Usage: /explanation <thought_id>   Example: /explanation latest".to_string(),
            );
            return Ok(());
        };

        self.cmd_transparency_query(
            "Reasoning chain",
            json!({"ReasoningChain": {"thought_id": thought_id}}),
        )
        .await
    }

    async fn cmd_transparency_query(
        &mut self,
        label: &str,
        payload: Value,
    ) -> Result<(), CliError> {
        match self.ipc.send("transparency_query", payload.clone()).await {
            Ok(response) => {
                // Try to parse the response into a TransparencyResult and format it
                let formatted = Self::format_transparency_result(&response);
                self.log_message(format!("{}:\n{}", label, formatted))
            }
            Err(e) => self.log_message(format!("Could not run transparency query: {}", e)),
        }
        Ok(())
    }

    async fn cmd_memory_query(&mut self, terms: &[&str]) -> Result<(), CliError> {
        if terms.is_empty() {
            self.log_message(
                "Usage: /query <text>   Example: /query recent model changes".to_string(),
            );
            return Ok(());
        }
        let query = terms.join(" ");
        match self.ipc.send("memory.query", json!({"query": query})).await {
            Ok(response) => self.log_message(format!("Memory results: {}", response)),
            Err(e) => self.log_message(format!("Could not search memory: {}", e)),
        }
        Ok(())
    }

    async fn cmd_events_subscribe(&mut self) -> Result<(), CliError> {
        match self.ipc.send("events.subscribe", json!({})).await {
            Ok(response) => self.log_message(format!("Event stream subscribed: {}", response)),
            Err(e) => self.log_message(format!("Could not subscribe to events: {}", e)),
        }
        Ok(())
    }

    async fn cmd_inference_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("inference.status", json!({})).await {
            Ok(response) => self.log_message(format!("Inference status: {}", response)),
            Err(e) => self.log_message(format!("Could not read inference status: {}", e)),
        }
        Ok(())
    }

    async fn cmd_speech_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("speech.status", json!({})).await {
            Ok(response) => self.log_message(format!("Speech status: {}", response)),
            Err(e) => self.log_message(format!("Could not read speech status: {}", e)),
        }
        Ok(())
    }

    async fn cmd_loops(&mut self, name: Option<&str>, state: Option<&str>) -> Result<(), CliError> {
        match (name, state) {
            (None, None) => match self.ipc.send("loops.list", json!({})).await {
                Ok(response) => {
                    self.log_message("Background loops:".to_string());
                    if let Some(loops_array) = response.get("loops").and_then(|v| v.as_array()) {
                        let mut display_lines = Vec::new();
                        let mut updates = Vec::new();

                        for loop_data in loops_array {
                            let name = loop_data
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let enabled = loop_data
                                .get("enabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let desc = loop_data
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let status = if enabled { "●" } else { "○" };
                            display_lines.push(format!("  {} {} — {}", status, name, desc));
                            updates.push((name.to_string(), desc.to_string(), enabled));
                        }

                        for line in display_lines {
                            self.log_message(line);
                        }

                        if let Ok(mut loops_map) = self.loops.lock() {
                            loops_map.clear();
                            for (name, description, enabled) in updates {
                                loops_map.insert(
                                    name.clone(),
                                    LoopInfo {
                                        name,
                                        description,
                                        enabled,
                                    },
                                );
                            }
                        }
                    }
                }
                Err(e) => self.log_message(format!("Could not list loops: {}", e)),
            },
            (Some(name), Some("on")) => {
                match self
                    .ipc
                    .send("loops.set", json!({"loop_name": name, "enabled": true}))
                    .await
                {
                    Ok(response) => {
                        self.log_message(format!("Loop '{}' enabled: {}", name, response))
                    }
                    Err(e) => self.log_message(format!("Could not enable loop '{}': {}", name, e)),
                }
            }
            (Some(name), Some("off")) => {
                match self
                    .ipc
                    .send("loops.set", json!({"loop_name": name, "enabled": false}))
                    .await
                {
                    Ok(response) => {
                        self.log_message(format!("Loop '{}' disabled: {}", name, response))
                    }
                    Err(e) => self.log_message(format!("Could not disable loop '{}': {}", name, e)),
                }
            }
            (Some(name), None) => {
                self.log_message(format!("Usage: /loops {} on|off", name));
            }
            (_, Some(invalid)) => {
                self.log_message(format!("Unknown loop state '{}'. Use on or off.", invalid));
            }
        }
        Ok(())
    }

    fn format_json_response(value: &Value) -> String {
        match serde_json::to_string_pretty(value) {
            Ok(formatted) => formatted,
            Err(_) => value.to_string(),
        }
    }

    fn format_transparency_result(value: &Value) -> String {
        use crate::transparency_format;

        // Try to parse the response into a structured type
        if let Some(observation) = value.get("Observation")
            && let Ok(resp) = serde_json::from_value::<bus::events::transparency::ObservationResponse>(
                observation.clone(),
            )
        {
            return transparency_format::format_observation_response(&resp);
        }

        if let Some(memory) = value.get("Memory")
            && let Ok(resp) =
                serde_json::from_value::<bus::events::transparency::MemoryResponse>(memory.clone())
        {
            return transparency_format::format_memory_response(&resp);
        }

        if let Some(reasoning) = value.get("Reasoning")
            && let Ok(resp) = serde_json::from_value::<bus::events::transparency::ReasoningResponse>(
                reasoning.clone(),
            )
        {
            return transparency_format::format_reasoning_response(&resp);
        }

        // Fallback to generic JSON formatting if structured parsing fails
        Self::format_json_response(value)
    }

    async fn open_config_editor(&mut self) -> Result<(), CliError> {
        self.cleanup_terminal()?;

        let mut editor = ConfigEditor::new(&mut self.ipc);
        editor.run().await?;

        enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let backend = CrosstermBackend::new(stdout);
        self.terminal =
            Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        self.log_message("Config editor closed".to_string());
        Ok(())
    }

    async fn handle_model_modal_enter(&mut self) -> Result<(), CliError> {
        let model = self.modal.as_ref().and_then(|modal| match modal {
            ModalState::Models(modal) => modal.selected().cloned(),
        });

        let Some(model) = model else {
            self.modal = None;
            return Ok(());
        };

        self.modal = None;
        self.log_message(format!("Loading model '{}'...", model.name));
        self.cmd_load_model(Some(model.path.as_str())).await
    }

    fn log_help_section(&mut self, title: &str, entries: &[(&str, &str, &str)]) {
        self.log_message(format!("{}:", title));
        for (command, description, example) in entries {
            self.log_message(format!(
                "  {:<26} {}  e.g. {}",
                command, description, example
            ));
        }
    }

    fn log_message(&mut self, message: String) {
        if let Ok(mut log) = self.message_log.lock() {
            log.push(message);
            if log.len() > 500 {
                log.drain(0..100);
            }
        }
    }

    fn render_tui(
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        render: ShellRenderState<'_>,
    ) -> Result<(), io::Error> {
        terminal.draw(|frame| {
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(0), Constraint::Length(30)])
                .split(frame.area());

            let left_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(0),
                    Constraint::Length(3),
                ])
                .split(main_chunks[0]);

            Self::render_header(
                frame,
                left_chunks[0],
                render.daemon_status,
                render.daemon_uptime_secs,
                render.vram,
            );
            Self::render_message_log(
                frame,
                left_chunks[1],
                render.message_log,
                render.log_scroll,
            );
            Self::render_input(frame, left_chunks[2], render.input_buffer);
            Self::render_loops_sidebar(frame, main_chunks[1], render.loops);

            if render.modal.is_none() {
                Self::render_slash_dropdown(frame, left_chunks[2], render.slash_dropdown);
            }
            if let Some(modal_state) = render.modal {
                Self::render_modal(frame, modal_state);
            }
        })?;
        Ok(())
    }

    fn render_header(
        frame: &mut Frame,
        area: Rect,
        daemon_status: &str,
        daemon_uptime_secs: u64,
        vram: Option<VramState>,
    ) {
        let mut spans = vec![
            Span::styled("SENA DEV", theme::title_style()),
            Span::styled("  uptime ", theme::muted()),
            Span::styled(format!("{}s", daemon_uptime_secs), theme::text()),
            Span::styled("  daemon ", theme::muted()),
            Span::styled(
                daemon_status,
                if daemon_status == "Connected" {
                    theme::success()
                } else {
                    theme::warning()
                },
            ),
        ];

        if let Some(vram_state) = vram
            && Instant::now().duration_since(vram_state.updated_at) <= Duration::from_secs(5)
            && vram_state.total_mb > 0
        {
            let bar = Self::vram_bar(vram_state.percent);
            let used_gb = vram_state.used_mb as f64 / 1024.0;
            let total_gb = vram_state.total_mb as f64 / 1024.0;
            let vram_style = if vram_state.percent < 70 {
                theme::success()
            } else if vram_state.percent <= 90 {
                theme::warning()
            } else {
                theme::danger()
            };

            spans.push(Span::styled("  VRAM ", theme::muted()));
            spans.push(Span::styled(
                format!("[{}] {:.1} / {:.1} GB", bar, used_gb, total_gb),
                vram_style,
            ));
        } else {
            spans.push(Span::styled("  VRAM ", theme::muted()));
            spans.push(Span::styled("[░░░░░░░░░░] n/a", theme::muted()));
        }

        let header = Paragraph::new(Line::from(spans)).block(theme::panel("Status"));

        frame.render_widget(header, area);
    }

    fn vram_bar(percent: u8) -> String {
        let filled = ((percent.min(100) as usize) * 10 + 50) / 100;
        let empty = 10usize.saturating_sub(filled);
        format!("{}{}", "█".repeat(filled), "░".repeat(empty))
    }

    fn render_message_log(frame: &mut Frame, area: Rect, message_log: &[String], scroll: usize) {
        let height = area.height.saturating_sub(2) as usize;
        let width = area.width.saturating_sub(2) as usize;
        let lines = message_log
            .iter()
            .flat_map(|message| {
                Self::wrap_log_message(message, width)
                    .into_iter()
                    .map(|line| Line::from(Span::styled(line, theme::log_line(message))))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let total = lines.len();
        let max_scroll = total.saturating_sub(height);
        let clamped_scroll = scroll.min(max_scroll);
        let top_scroll = total.saturating_sub(height + clamped_scroll);

        let title = if clamped_scroll > 0 {
            format!("Messages (↑{} lines)", clamped_scroll)
        } else {
            "Messages".to_string()
        };

        let para = Paragraph::new(lines)
            .block(theme::panel(&title))
            .wrap(Wrap { trim: false })
            .scroll((top_scroll as u16, 0));

        frame.render_widget(para, area);
    }

    fn wrap_log_message(message: &str, max_width: usize) -> Vec<String> {
        if max_width == 0 {
            return vec![String::new()];
        }

        let mut wrapped = Vec::new();
        for raw_line in message.lines() {
            if raw_line.is_empty() {
                wrapped.push(String::new());
                continue;
            }

            let mut current = String::new();
            for word in raw_line.split_whitespace() {
                let candidate_len = if current.is_empty() {
                    word.chars().count()
                } else {
                    current.chars().count() + 1 + word.chars().count()
                };

                if candidate_len <= max_width {
                    if !current.is_empty() {
                        current.push(' ');
                    }
                    current.push_str(word);
                    continue;
                }

                if !current.is_empty() {
                    wrapped.push(current);
                }

                let mut chunk = String::new();
                for ch in word.chars() {
                    if chunk.chars().count() == max_width {
                        wrapped.push(chunk);
                        chunk = String::new();
                    }
                    chunk.push(ch);
                }
                current = chunk;
            }

            if !current.is_empty() {
                wrapped.push(current);
            }
        }

        if wrapped.is_empty() {
            wrapped.push(String::new());
        }

        wrapped
    }

    fn render_input(frame: &mut Frame, area: Rect, input_buffer: &str) {
        let input = Paragraph::new(input_buffer)
            .block(theme::focused_panel("Input"))
            .style(theme::text());

        frame.render_widget(input, area);
    }

    fn render_loops_sidebar(frame: &mut Frame, area: Rect, loops: &[LoopInfo]) {
        let mut sorted = loops.to_vec();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));

        let items: Vec<ListItem> = sorted
            .iter()
            .map(|loop_info| {
                let dot = if loop_info.enabled {
                    Span::styled("● ", theme::success())
                } else {
                    Span::styled("● ", theme::danger())
                };
                let text = if loop_info.description.is_empty() {
                    loop_info.name.clone()
                } else {
                    format!("{} — {}", loop_info.name, loop_info.description)
                };
                ListItem::new(Line::from(vec![dot, Span::styled(text, theme::text())]))
            })
            .collect();

        let loops_widget = List::new(items).block(theme::panel("Loops"));

        frame.render_widget(loops_widget, area);
    }

    fn render_slash_dropdown(
        frame: &mut Frame,
        input_area: Rect,
        slash_dropdown: Option<&SlashDropdown>,
    ) {
        let Some(dropdown) = slash_dropdown else {
            return;
        };

        if dropdown.no_matches {
            let popup_area = Rect {
                x: input_area.x + 1,
                y: input_area.y.saturating_sub(3),
                width: 34u16.min(frame.area().width.saturating_sub(2)),
                height: 3,
            };
            frame.render_widget(Clear, popup_area);
            let panel = Paragraph::new(Line::from(Span::styled(
                "No matching commands",
                theme::muted(),
            )))
            .block(theme::panel("Command Helper"));
            frame.render_widget(panel, popup_area);
            return;
        }

        if dropdown.is_empty() {
            return;
        }

        let visible_count = dropdown.filtered.len().min(6);
        let popup_area = Rect {
            x: input_area.x + 1,
            y: input_area.y.saturating_sub((visible_count + 2) as u16),
            width: 58u16.min(frame.area().width.saturating_sub(2)),
            height: (visible_count + 2) as u16,
        };
        frame.render_widget(Clear, popup_area);

        let items = dropdown
            .filtered
            .iter()
            .take(visible_count)
            .map(|&index| {
                let command = &SLASH_COMMANDS[index];
                ListItem::new(Line::from(vec![
                    Span::styled(command.command, theme::title_style()),
                    Span::styled("  ", theme::text()),
                    Span::styled(format!("[{}]", command.category.label()), theme::muted()),
                    Span::styled("  ", theme::text()),
                    Span::styled(command.description, theme::text()),
                ]))
            })
            .collect::<Vec<_>>();

        let mut state = ListState::default();
        state.select(Some(dropdown.selected.min(visible_count.saturating_sub(1))));
        let list = List::new(items)
            .block(theme::focused_panel("Command Helper"))
            .highlight_style(theme::selected());
        frame.render_stateful_widget(list, popup_area, &mut state);
    }

    fn render_modal(frame: &mut Frame, modal: &ModalState) {
        match modal {
            ModalState::Models(model_modal) => Self::render_model_modal(frame, model_modal),
        }
    }

    fn render_model_modal(frame: &mut Frame, model_modal: &ModelModal) {
        let area = Self::centered_rect(68, 60, frame.area());
        frame.render_widget(Clear, area);

        let items = model_modal
            .models
            .iter()
            .map(|model| {
                let size_gb = model.size_bytes as f64 / 1_073_741_824.0;
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:<28}", model.name), theme::text()),
                    Span::styled(format!("{:>5.1} GB", size_gb), theme::muted()),
                ]))
            })
            .collect::<Vec<_>>();

        let mut state = ListState::default();
        state.select(Some(model_modal.selected));
        let list = List::new(items)
            .block(theme::focused_panel(
                "Models (↑↓ navigate, Enter select, Esc cancel)",
            ))
            .highlight_style(theme::selected());
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ])
            .split(area);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(vertical[1])[1]
    }

    fn cleanup_terminal(&mut self) -> Result<(), CliError> {
        disable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        self.terminal
            .show_cursor()
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        Ok(())
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    }
}

#[cfg(test)]
mod tests {
    use super::Shell;
    use serde_json::json;

    #[test]
    fn listen_mode_push_events_replace_live_partial_and_finalize_cleanly() {
        let events = [
            json!({
                "type": "ListenModeTranscription",
                "data": { "text": "hello" }
            }),
            json!({
                "type": "ListenModeTranscription",
                "data": { "text": "hello world" }
            }),
            json!({
                "type": "ListenModeTranscription",
                "data": { "text": "hello world from" }
            }),
            json!({
                "type": "ListenModeTranscriptFinalized",
                "data": { "text": "hello world from sena" }
            }),
        ];

        let mut log = Vec::new();

        let first = Shell::format_push_event(&events[0]).expect("first partial should format");
        Shell::append_push_line(&mut log, first);
        assert_eq!(log, vec!["[STT~] hello".to_string()]);

        let second = Shell::format_push_event(&events[1]).expect("second partial should format");
        Shell::append_push_line(&mut log, second);
        assert_eq!(log, vec!["[STT~] hello world".to_string()]);

        let third = Shell::format_push_event(&events[2]).expect("third partial should format");
        Shell::append_push_line(&mut log, third);
        assert_eq!(log, vec!["[STT~] hello world from".to_string()]);

        let final_line =
            Shell::format_push_event(&events[3]).expect("final transcript should format");
        Shell::append_push_line(&mut log, final_line);
        assert_eq!(log, vec!["[STT] \"hello world from sena\"".to_string()]);
    }

    #[test]
    fn low_confidence_push_event_formats_as_unclear() {
        let event = json!({
            "type": "LowConfidenceTranscription",
            "data": {
                "text": "maybe hello",
                "confidence": 0.41
            }
        });

        let line = Shell::format_push_event(&event).expect("low confidence event should format");

        assert_eq!(line, "[unclear] \"maybe hello\" (conf: 0.41)");
    }

    #[test]
    fn wakeword_push_events_format_cleanly() {
        let detected = json!({
            "type": "WakewordDetected",
            "data": {
                "confidence": 0.82
            }
        });
        let suppressed = json!({
            "type": "WakewordSuppressed",
            "data": {
                "reason": "listen mode active"
            }
        });
        let resumed = json!({
            "type": "WakewordResumed",
            "data": {}
        });

        assert_eq!(
            Shell::format_push_event(&detected).expect("wakeword detected should format"),
            "[wakeword] detected (conf: 0.82)"
        );
        assert_eq!(
            Shell::format_push_event(&suppressed).expect("wakeword suppressed should format"),
            "[wakeword] suppressed (listen mode active)"
        );
        assert_eq!(
            Shell::format_push_event(&resumed).expect("wakeword resumed should format"),
            "[wakeword] resumed"
        );
    }
}
