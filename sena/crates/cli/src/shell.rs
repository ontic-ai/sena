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
    style::Style,
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::{Value, json};
use sri::{HealthStatus, RegisteredSriNode, ResourceKind, SignalSource, SriEvent, SriResourceSnapshot, SriSnapshot, SriTreeNode, TreeAction};
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::info;

#[derive(Clone, Debug)]
struct LoopInfo {
    enabled: bool,
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
    (
        "/say \"text\"",
        "speak text verbatim through TTS (audio test)",
        "/say \"hello world\"",
    ),
    (
        "/run \"text\"",
        "run full inference pipeline as if spoken",
        "/run \"what time is it\"",
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
    ("/tree", "toggle live vs full tree expansion", "/tree"),
    ("/sri", "dump the current SRI snapshot", "/sri"),
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
        command: "/say",
        description: "Speak text verbatim through TTS",
        category: CommandCategory::Speech,
    },
    SlashCommand {
        command: "/run",
        description: "Run full inference pipeline as if spoken",
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
        command: "/tree",
        description: "Toggle live vs full tree expansion",
        category: CommandCategory::Runtime,
    },
    SlashCommand {
        command: "/sri",
        description: "Dump the current SRI snapshot",
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

#[derive(Clone, Debug, Default)]
struct SriPanelState {
    snapshot: Option<SriSnapshot>,
    active_shelf: Option<String>,
}

struct ShellRenderState<'a> {
    message_log: &'a [String],
    response_log: &'a [String],
    input_buffer: &'a str,
    daemon_status: &'a str,
    daemon_uptime_secs: u64,
    log_scroll: usize,
    sri_panel: Option<&'a SriPanelState>,
    full_tree: bool,
    slash_dropdown: Option<&'a SlashDropdown>,
    modal: Option<&'a ModalState>,
}

pub struct Shell {
    ipc: IpcClient,
    message_log: Arc<Mutex<Vec<String>>>,
    response_log: Arc<Mutex<Vec<String>>>,
    sri_panel: Arc<Mutex<SriPanelState>>,
    loops: Arc<Mutex<HashMap<String, LoopInfo>>>,
    input_buffer: String,
    should_quit: bool,
    daemon_status: String,
    daemon_uptime_secs: u64,
    daemon_uptime_anchor: Instant,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    connection_alive: Arc<AtomicBool>,
    log_scroll: usize,
    quit_armed: bool,
    full_tree: bool,
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
        let response_log = Arc::new(Mutex::new(vec![
            "[LLM] waiting for live response stream".to_string(),
        ]));
        let sri_panel = Arc::new(Mutex::new(SriPanelState::default()));
        let loops: Arc<Mutex<HashMap<String, LoopInfo>>> = Arc::new(Mutex::new(HashMap::new()));

        let mut daemon_uptime_secs = 0;

        match ipc.send("sri.subscribe", json!({})).await {
            Ok(_) => {
                if let Ok(mut log) = message_log.lock() {
                    log.push("[SYS] subscribed to SRI stream".to_string());
                }
            }
            Err(e) => {
                if let Ok(mut log) = message_log.lock() {
                    log.push(format!("[ERR] sri.subscribe failed: {}", e));
                }
            }
        }

        match ipc.send("sri.snapshot", json!({})).await {
            Ok(response) => {
                if let Some(snapshot) = response.get("snapshot").cloned() {
                    match serde_json::from_value::<SriSnapshot>(snapshot) {
                        Ok(snapshot) => {
                            if let Ok(mut panel) = sri_panel.lock() {
                                Self::set_snapshot(&mut panel, snapshot);
                            }
                            if let Ok(mut log) = message_log.lock() {
                                log.push("[SYS] loaded initial SRI snapshot".to_string());
                            }
                        }
                        Err(error) => {
                            if let Ok(mut log) = message_log.lock() {
                                log.push(format!("[ERR] invalid sri.snapshot payload: {}", error));
                            }
                        }
                    }
                }
            }
            Err(error) => {
                if let Ok(mut log) = message_log.lock() {
                    log.push(format!("[ERR] sri.snapshot failed: {}", error));
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
                let _description = loop_data
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
                    LoopInfo { enabled },
                );
            }
        }

        let push_log = Arc::clone(&message_log);
        let push_response_log = Arc::clone(&response_log);
        let push_sri_panel = Arc::clone(&sri_panel);
        let push_loops = Arc::clone(&loops);
        let connection_alive = Arc::new(AtomicBool::new(true));
        let connection_alive_task = Arc::clone(&connection_alive);
        let mut push_rx = ipc.subscribe_events();

        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                let stream = event
                    .get("stream")
                    .and_then(|value| value.as_str())
                    .unwrap_or("events");

                if stream == "sri" {
                    let Some(event_value) = event.get("event").cloned() else {
                        continue;
                    };

                    match serde_json::from_value::<SriEvent>(event_value) {
                        Ok(sri_event) => {
                            let maybe_signal = if let (Ok(mut panel), Ok(mut response_log)) = (
                                push_sri_panel.lock(),
                                push_response_log.lock(),
                            ) {
                                Self::apply_sri_event(&mut panel, &mut response_log, sri_event)
                            } else {
                                None
                            };

                            if let Some(line) = maybe_signal
                                && let Ok(mut log) = push_log.lock()
                            {
                                Self::append_push_line(&mut log, line);
                            }
                        }
                        Err(error) => {
                            if let Ok(mut log) = push_log.lock() {
                                Self::append_push_line(
                                    &mut log,
                                    format!("[ERR] could not decode sri event: {}", error),
                                );
                            }
                        }
                    }
                    continue;
                }

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
            response_log,
            sri_panel,
            loops,
            input_buffer: String::new(),
            should_quit: false,
            daemon_status: "Connected".to_string(),
            daemon_uptime_secs,
            daemon_uptime_anchor: Instant::now(),
            terminal,
            connection_alive,
            log_scroll: 0,
            quit_armed: false,
            full_tree: false,
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

        if let (Ok(log), Ok(response_log), Ok(panel)) = (
            self.message_log.lock(),
            self.response_log.lock(),
            self.sri_panel.lock(),
        ) {
            let uptime_secs = self.current_uptime_secs();
            Self::render_tui(
                &mut self.terminal,
                ShellRenderState {
                    message_log: &log,
                    response_log: &response_log,
                    input_buffer: &self.input_buffer,
                    daemon_status: &self.daemon_status,
                    daemon_uptime_secs: uptime_secs,
                    log_scroll: self.log_scroll,
                    sri_panel: Some(&panel),
                    full_tree: self.full_tree,
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

            if let (Ok(log), Ok(response_log), Ok(panel)) = (
                self.message_log.lock(),
                self.response_log.lock(),
                self.sri_panel.lock(),
            ) {
                let uptime_secs = self.current_uptime_secs();
                Self::render_tui(
                    &mut self.terminal,
                    ShellRenderState {
                        message_log: &log,
                        response_log: &response_log,
                        input_buffer: &self.input_buffer,
                        daemon_status: &self.daemon_status,
                        daemon_uptime_secs: uptime_secs,
                        log_scroll: self.log_scroll,
                        sri_panel: Some(&panel),
                        full_tree: self.full_tree,
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

    fn parse_quoted_command_text(input: &str, command: &str) -> Option<String> {
        let remainder = input.trim().strip_prefix(command)?.trim();
        let text = remainder.strip_prefix('"')?.strip_suffix('"')?;

        if text.trim().is_empty() {
            None
        } else {
            Some(text.to_string())
        }
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
            "/say" => self.cmd_say(input).await?,
            "/run" => self.cmd_run(input).await?,
            "/observation" | "/obs" => self.cmd_observation().await?,
            "/memory" | "/mem" => self.cmd_transparency_memory().await?,
            "/memory-stats" | "/memstats" => self.cmd_memory_stats().await?,
            "/explanation" | "/explain" => self.cmd_explanation(&parts[1..]).await?,
            "/query" | "/search" | "/recall" => self.cmd_memory_query(&parts[1..]).await?,
            "/config" | "/settings" => self.open_config_editor().await?,
            "/tree" => self.cmd_tree_toggle(),
            "/sri" => self.cmd_sri_snapshot().await?,
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

    async fn cmd_say(&mut self, input: &str) -> Result<(), CliError> {
        let Some(text) = Self::parse_quoted_command_text(input, "/say") else {
            self.log_message("usage: /say \"text to speak\"".to_string());
            return Ok(());
        };

        match self.ipc.send("speech.say", json!({"text": text})).await {
            Ok(_) => self.log_message(format!("[SAY] \"{}\"", text)),
            Err(e) => self.log_message(format!("Could not send speech.say: {}", e)),
        }

        Ok(())
    }

    async fn cmd_run(&mut self, input: &str) -> Result<(), CliError> {
        let Some(text) = Self::parse_quoted_command_text(input, "/run") else {
            self.log_message("usage: /run \"text to process\"".to_string());
            return Ok(());
        };

        match self.ipc.send("inference.run", json!({"text": text})).await {
            Ok(_) => self.log_message(format!("[RUN] \"{}\"", text)),
            Err(e) => self.log_message(format!("Could not send inference.run: {}", e)),
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
                                let _ = description;
                                loops_map.insert(
                                    name.clone(),
                                    LoopInfo { enabled },
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

    fn cmd_tree_toggle(&mut self) {
        self.full_tree = !self.full_tree;
        let mode = if self.full_tree { "full" } else { "live" };
        self.log_message(format!("[SYS] tree mode set to {}", mode));
    }

    async fn cmd_sri_snapshot(&mut self) -> Result<(), CliError> {
        match self.ipc.send("sri.snapshot", json!({})).await {
            Ok(response) => {
                let Some(snapshot_value) = response.get("snapshot").cloned() else {
                    self.log_message("[ERR] sri.snapshot returned no snapshot".to_string());
                    return Ok(());
                };

                match serde_json::from_value::<SriSnapshot>(snapshot_value.clone()) {
                    Ok(snapshot) => {
                        if let Ok(mut panel) = self.sri_panel.lock() {
                            Self::set_snapshot(&mut panel, snapshot);
                        }

                        match serde_json::to_string_pretty(&snapshot_value) {
                            Ok(pretty) => {
                                for line in pretty.lines() {
                                    self.log_message(format!("[SRI] {}", line));
                                }
                            }
                            Err(error) => self.log_message(format!(
                                "[ERR] could not render sri.snapshot: {}",
                                error
                            )),
                        }
                    }
                    Err(error) => {
                        self.log_message(format!("[ERR] invalid sri.snapshot payload: {}", error))
                    }
                }
            }
            Err(error) => self.log_message(format!("[ERR] sri.snapshot failed: {}", error)),
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

    fn set_snapshot(panel: &mut SriPanelState, snapshot: SriSnapshot) {
        panel.active_shelf = Self::select_active_shelf(&snapshot, panel.active_shelf.as_deref());
        panel.snapshot = Some(snapshot);
    }

    fn select_active_shelf(snapshot: &SriSnapshot, preferred: Option<&str>) -> Option<String> {
        if let Some(preferred) = preferred
            && snapshot.open_shelves.iter().any(|path| path == preferred)
        {
            return Some(preferred.to_string());
        }

        snapshot
            .open_shelves
            .last()
            .cloned()
            .or_else(|| snapshot.nodes.first().map(|node| node.shelf_path.clone()))
    }

    fn apply_sri_event(
        panel: &mut SriPanelState,
        response_log: &mut Vec<String>,
        event: SriEvent,
    ) -> Option<String> {
        match event {
            SriEvent::TreeSnapshot { tree } => {
                if let Some(snapshot) = panel.snapshot.as_mut() {
                    snapshot.tree = tree;
                    panel.active_shelf = Self::select_active_shelf(snapshot, panel.active_shelf.as_deref());
                } else {
                    panel.snapshot = Some(SriSnapshot {
                        tree,
                        nodes: Vec::new(),
                        open_shelves: Vec::new(),
                        latest_resources: None,
                    });
                }
                None
            }
            SriEvent::TreeNavigation {
                shelf_path, action, ..
            } => {
                if let Some(snapshot) = panel.snapshot.as_mut() {
                    match action {
                        TreeAction::Open => {
                            if !snapshot.open_shelves.iter().any(|path| path == &shelf_path) {
                                snapshot.open_shelves.push(shelf_path.clone());
                            }
                            panel.active_shelf = Some(shelf_path);
                        }
                        TreeAction::Close => {
                            snapshot.open_shelves.retain(|path| path != &shelf_path);
                            let preferred = panel
                                .active_shelf
                                .as_deref()
                                .filter(|path| *path != shelf_path.as_str());
                            panel.active_shelf = Self::select_active_shelf(snapshot, preferred);
                        }
                    }
                }
                None
            }
            SriEvent::SignalReceived {
                source,
                summary,
                timestamp,
            } => {
                if let Some(response_line) = Self::map_response_line(source, &summary) {
                    Self::append_response_line(response_log, response_line);
                }
                Some(Self::format_signal_line(
                    source,
                    &summary,
                    timestamp.format("%H:%M:%S").to_string(),
                ))
            }
            SriEvent::NodeHealthChanged {
                shelf_path,
                old: _,
                new,
            } => {
                if let Some(snapshot) = panel.snapshot.as_mut() {
                    Self::apply_snapshot_health(snapshot, &shelf_path, new);
                }
                Some(format!(
                    "[HEALTH] {} -> {}",
                    shelf_path,
                    Self::health_label(new)
                ))
            }
            SriEvent::FunctionCallStub { path, .. } => {
                Some(format!("[CAPABILITY] {} -> unavailable", path))
            }
            SriEvent::ResourceSnapshot(snapshot) => {
                if let Some(current) = panel.snapshot.as_mut() {
                    current.latest_resources = Some(snapshot);
                } else {
                    panel.snapshot = Some(SriSnapshot {
                        tree: SriTreeNode::root(),
                        nodes: Vec::new(),
                        open_shelves: Vec::new(),
                        latest_resources: Some(snapshot),
                    });
                }
                None
            }
            SriEvent::ResourceAlert {
                actor,
                resource,
                value,
                threshold,
            } => Some(format!(
                "[FAULT] {} {} {:.1} > {:.1}",
                actor,
                Self::resource_label(resource),
                value,
                threshold
            )),
        }
    }

    fn apply_snapshot_health(snapshot: &mut SriSnapshot, shelf_path: &str, new: HealthStatus) {
        if let Some(node) = snapshot
            .nodes
            .iter_mut()
            .find(|node| node.shelf_path == shelf_path)
        {
            node.health_status = new;
        }

        Self::refresh_tree_health(&mut snapshot.tree, &snapshot.nodes);
    }

    fn refresh_tree_health(node: &mut SriTreeNode, nodes: &[RegisteredSriNode]) -> HealthStatus {
        for child in &mut node.children {
            Self::refresh_tree_health(child, nodes);
        }

        let own = if node.registered {
            nodes.iter()
                .find(|registered| registered.shelf_path == node.shelf_path)
                .map(|registered| registered.health_status)
                .or(Some(node.health_status))
        } else {
            None
        };

        node.health_status = Self::aggregate_health(own, &node.children);
        node.health_status
    }

    fn aggregate_health(own: Option<HealthStatus>, children: &[SriTreeNode]) -> HealthStatus {
        let mut saw_degraded = matches!(own, Some(HealthStatus::Degraded));
        if matches!(own, Some(HealthStatus::Active)) {
            return HealthStatus::Active;
        }

        for child in children {
            match child.health_status {
                HealthStatus::Active => return HealthStatus::Active,
                HealthStatus::Degraded => saw_degraded = true,
                HealthStatus::Unavailable => {}
            }
        }

        if saw_degraded {
            HealthStatus::Degraded
        } else {
            HealthStatus::Unavailable
        }
    }

    fn append_response_line(log: &mut Vec<String>, line: String) {
        log.push(line);
        if log.len() > 240 {
            log.drain(0..40);
        }
    }

    fn health_label(status: HealthStatus) -> &'static str {
        match status {
            HealthStatus::Active => "active",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Unavailable => "unavailable",
        }
    }

    fn health_badge(status: HealthStatus) -> &'static str {
        match status {
            HealthStatus::Active => "[●]",
            HealthStatus::Degraded => "[~]",
            HealthStatus::Unavailable => "[x]",
        }
    }

    fn health_style(status: HealthStatus) -> Style {
        match status {
            HealthStatus::Active => theme::success(),
            HealthStatus::Degraded => theme::warning(),
            HealthStatus::Unavailable => theme::danger(),
        }
    }

    fn signal_source_label(source: SignalSource) -> &'static str {
        match source {
            SignalSource::Identity => "IDENTITY",
            SignalSource::Perception => "PERCEPTION",
            SignalSource::Cognition => "COGNITION",
            SignalSource::Expression => "EXPRESSION",
            SignalSource::Environment => "ENVIRONMENT",
            SignalSource::Fault => "FAULT",
        }
    }

    fn format_signal_line(
        source: SignalSource,
        summary: &str,
        timestamp: impl std::fmt::Display,
    ) -> String {
        format!(
            "[{}] {} · {}",
            Self::signal_source_label(source),
            summary,
            timestamp
        )
    }

    fn map_response_line(source: SignalSource, summary: &str) -> Option<String> {
        let lower = summary.to_ascii_lowercase();

        match source {
            SignalSource::Perception => Some(format!("[STT] {}", summary)),
            SignalSource::Cognition => {
                if lower.contains("memory") || lower.contains("context") {
                    Some(format!("[MEM] {}", summary))
                } else {
                    Some(format!("[CTP] {}", summary))
                }
            }
            SignalSource::Expression => {
                if lower.contains("voice playback") || lower.contains("speaking") {
                    Some(format!("[TTS] {}", summary))
                } else {
                    Some(format!("[LLM] {}", summary))
                }
            }
            SignalSource::Identity => Some(format!("[SOUL] {}", summary)),
            SignalSource::Fault => Some(format!("[FAULT] {}", summary)),
            SignalSource::Environment => None,
        }
    }

    fn render_tui(
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        render: ShellRenderState<'_>,
    ) -> Result<(), io::Error> {
        terminal.draw(|frame| {
            let vertical = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(0),
                    Constraint::Length(6),
                    Constraint::Length(3),
                ])
                .split(frame.area());

            let top = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(31),
                    Constraint::Percentage(37),
                    Constraint::Percentage(32),
                ])
                .split(vertical[0]);

            Self::render_tree_panel(frame, top[0], render.sri_panel, render.full_tree);
            Self::render_signal_panel(frame, top[1], render.message_log, render.log_scroll);
            Self::render_response_panel(frame, top[2], render.response_log);
            Self::render_resources_panel(
                frame,
                vertical[1],
                render.sri_panel,
                render.daemon_status,
                render.daemon_uptime_secs,
            );
            Self::render_input(
                frame,
                vertical[2],
                render.input_buffer,
                render.daemon_status,
                render.daemon_uptime_secs,
            );

            if render.modal.is_none() {
                Self::render_slash_dropdown(frame, vertical[2], render.slash_dropdown);
            }
            if let Some(modal_state) = render.modal {
                Self::render_modal(frame, modal_state);
            }
        })?;
        Ok(())
    }

    fn render_tree_panel(
        frame: &mut Frame,
        area: Rect,
        sri_panel: Option<&SriPanelState>,
        full_tree: bool,
    ) {
        let title = if full_tree { "Capability Tree [ALL]" } else { "Capability Tree [LIVE]" };
        let lines = if let Some(snapshot) = sri_panel.and_then(|panel| panel.snapshot.as_ref()) {
            let open_shelves = snapshot
                .open_shelves
                .iter()
                .cloned()
                .collect::<HashSet<_>>();
            let mut lines = vec![Line::from(Span::styled("SELF", theme::title_style()))];

            for (index, child) in snapshot.tree.children.iter().enumerate() {
                Self::push_tree_lines(
                    &mut lines,
                    child,
                    "",
                    index + 1 == snapshot.tree.children.len(),
                    &open_shelves,
                    full_tree,
                    sri_panel.and_then(|panel| panel.active_shelf.as_deref()),
                );
            }

            if lines.len() == 1 {
                lines.push(Line::from(Span::styled(
                    "waiting for registered capabilities...",
                    theme::muted(),
                )));
            }

            lines
        } else {
            vec![Line::from(Span::styled(
                "waiting for sri snapshot...",
                theme::muted(),
            ))]
        };

        let panel = Paragraph::new(lines)
            .block(theme::panel(title))
            .wrap(Wrap { trim: false });
        frame.render_widget(panel, area);
    }

    fn push_tree_lines(
        lines: &mut Vec<Line<'static>>,
        node: &SriTreeNode,
        prefix: &str,
        is_last: bool,
        open_shelves: &HashSet<String>,
        full_tree: bool,
        active_shelf: Option<&str>,
    ) {
        let has_children = !node.children.is_empty();
        let expanded = full_tree || open_shelves.contains(&node.shelf_path);
        let connector = if is_last { "└" } else { "├" };
        let marker = if has_children {
            if expanded { "▼" } else { "▶" }
        } else {
            "•"
        };
        let is_active = active_shelf == Some(node.shelf_path.as_str());
        let show_description = (expanded || is_active) && !node.description.is_empty();

        let mut spans = vec![
            Span::styled(format!("{}{}{} ", prefix, connector, marker), theme::muted()),
            Span::styled(
                node.display_name.clone(),
                if is_active { theme::title_style() } else { theme::text() },
            ),
            Span::styled(
                format!(" {}", Self::health_badge(node.health_status)),
                Self::health_style(node.health_status),
            ),
        ];

        if show_description {
            spans.push(Span::styled(
                format!(" {}", Self::truncate_chars(&node.description, 34)),
                theme::muted(),
            ));
        }

        lines.push(Line::from(spans));

        if has_children && expanded {
            let next_prefix = format!("{}{}  ", prefix, if is_last { " " } else { "│" });
            for (index, child) in node.children.iter().enumerate() {
                Self::push_tree_lines(
                    lines,
                    child,
                    &next_prefix,
                    index + 1 == node.children.len(),
                    open_shelves,
                    full_tree,
                    active_shelf,
                );
            }
        }
    }

    fn render_signal_panel(frame: &mut Frame, area: Rect, message_log: &[String], scroll: usize) {
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
            format!("Signals (↑{} lines)", clamped_scroll)
        } else {
            "Signals".to_string()
        };

        let para = Paragraph::new(lines)
            .block(theme::panel(&title))
            .wrap(Wrap { trim: false })
            .scroll((top_scroll as u16, 0));

        frame.render_widget(para, area);
    }

    fn render_response_panel(frame: &mut Frame, area: Rect, response_log: &[String]) {
        let height = area.height.saturating_sub(2) as usize;
        let width = area.width.saturating_sub(2) as usize;
        let wrapped = response_log
            .iter()
            .flat_map(|line| {
                Self::wrap_log_message(line, width)
                    .into_iter()
                    .map(|wrapped| Line::from(Span::styled(wrapped, Self::response_style(line))))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let start = wrapped.len().saturating_sub(height);
        let visible = wrapped.into_iter().skip(start).collect::<Vec<_>>();
        let para = Paragraph::new(visible)
            .block(theme::panel("Response"))
            .wrap(Wrap { trim: false });

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

    fn render_resources_panel(
        frame: &mut Frame,
        area: Rect,
        sri_panel: Option<&SriPanelState>,
        daemon_status: &str,
        daemon_uptime_secs: u64,
    ) {
        let title = format!("Resources · {} · {}s", daemon_status, daemon_uptime_secs);
        let lines = if let Some(snapshot) = sri_panel
            .and_then(|panel| panel.snapshot.as_ref())
            .and_then(|snapshot| snapshot.latest_resources.as_ref())
        {
            Self::resource_lines(snapshot)
        } else {
            vec![Line::from(Span::styled(
                "waiting for sri resource samples...",
                theme::muted(),
            ))]
        };

        let para = Paragraph::new(lines)
            .block(theme::panel(&title))
            .wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }

    fn resource_lines(snapshot: &SriResourceSnapshot) -> Vec<Line<'static>> {
        let mut lines = vec![Self::resource_line(
            "RAM",
            (snapshot.total_ram_mb as f32 / 1024.0 * 100.0).clamp(0.0, 100.0),
            Self::format_mb(snapshot.total_ram_mb),
            Self::dominant_ram(snapshot),
        )];

        lines.push(Self::resource_line(
            "CPU",
            snapshot.total_cpu_pct.clamp(0.0, 100.0),
            format!("{:.1}%", snapshot.total_cpu_pct),
            Self::dominant_cpu(snapshot),
        ));

        if let (Some(used_mb), Some(total_mb)) = (snapshot.vram_used_mb, snapshot.vram_total_mb) {
            let percent = if total_mb == 0 {
                0.0
            } else {
                used_mb as f32 / total_mb as f32 * 100.0
            };
            lines.push(Self::resource_line(
                "VRAM",
                percent.clamp(0.0, 100.0),
                format!("{} / {}", Self::format_mb(used_mb), Self::format_mb(total_mb)),
                Self::dominant_vram(snapshot),
            ));
        }

        lines
    }

    fn resource_line(label: &str, percent: f32, total: String, dominant: String) -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{:<4}", label), theme::muted()),
            Span::styled(
                format!("[{}] {}", Self::resource_bar(percent, 10), total),
                Self::resource_style(percent),
            ),
            Span::styled(format!("  top: {}", dominant), theme::muted()),
        ])
    }

    fn resource_bar(percent: f32, width: usize) -> String {
        let clamped = percent.clamp(0.0, 100.0);
        let filled = ((clamped / 100.0) * width as f32).round() as usize;
        let empty = width.saturating_sub(filled.min(width));
        format!("{}{}", "█".repeat(filled.min(width)), "░".repeat(empty))
    }

    fn resource_style(percent: f32) -> Style {
        if percent < 70.0 {
            theme::success()
        } else if percent < 90.0 {
            theme::warning()
        } else {
            theme::danger()
        }
    }

    fn format_mb(value_mb: u64) -> String {
        if value_mb >= 1024 {
            format!("{:.2} GB", value_mb as f64 / 1024.0)
        } else {
            format!("{} MB", value_mb)
        }
    }

    fn dominant_ram(snapshot: &SriResourceSnapshot) -> String {
        snapshot
            .actors
            .iter()
            .max_by_key(|actor| actor.ram_mb)
            .map(|actor| format!("{} {}", actor.actor_name, Self::format_mb(actor.ram_mb)))
            .unwrap_or_else(|| "n/a".to_string())
    }

    fn dominant_cpu(snapshot: &SriResourceSnapshot) -> String {
        snapshot
            .actors
            .iter()
            .max_by(|left, right| left.cpu_pct.total_cmp(&right.cpu_pct))
            .map(|actor| format!("{} {:.1}%", actor.actor_name, actor.cpu_pct))
            .unwrap_or_else(|| "n/a".to_string())
    }

    fn dominant_vram(snapshot: &SriResourceSnapshot) -> String {
        snapshot
            .actors
            .iter()
            .filter_map(|actor| actor.vram_pct.map(|value| (actor.actor_name.as_str(), value)))
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(actor, value)| format!("{} {:.1}%", actor, value))
            .unwrap_or_else(|| "n/a".to_string())
    }

    fn response_style(line: &str) -> Style {
        if line.starts_with("[TTS]") || line.starts_with("[SOUL]") {
            theme::warning()
        } else if line.starts_with("[MEM]") || line.starts_with("[CTP]") {
            theme::muted()
        } else if line.starts_with("[FAULT]") {
            theme::danger()
        } else if line.starts_with("[LLM]") {
            theme::success()
        } else {
            theme::text()
        }
    }

    fn resource_label(resource: ResourceKind) -> &'static str {
        match resource {
            ResourceKind::Ram => "ram",
            ResourceKind::Cpu => "cpu",
            ResourceKind::Vram => "vram",
        }
    }

    fn truncate_chars(text: &str, max_chars: usize) -> String {
        let count = text.chars().count();
        if count <= max_chars {
            return text.to_string();
        }

        let visible = max_chars.saturating_sub(1);
        format!("{}…", text.chars().take(visible).collect::<String>())
    }

    fn render_input(
        frame: &mut Frame,
        area: Rect,
        input_buffer: &str,
        daemon_status: &str,
        daemon_uptime_secs: u64,
    ) {
        let title = format!("Input · {} · {}s", daemon_status, daemon_uptime_secs);
        let input = Paragraph::new(input_buffer)
            .block(theme::focused_panel(&title))
            .style(theme::text());

        frame.render_widget(input, area);
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
    use super::{HELP_SPEECH, SLASH_COMMANDS, Shell};
    use serde_json::json;

    #[test]
    fn quoted_command_text_parses_balanced_quotes_only() {
        assert_eq!(
            Shell::parse_quoted_command_text("/say \"hello world\"", "/say"),
            Some("hello world".to_string())
        );
        assert_eq!(
            Shell::parse_quoted_command_text("/run \"  hi there  \"", "/run"),
            Some("  hi there  ".to_string())
        );

        assert_eq!(Shell::parse_quoted_command_text("/say", "/say"), None);
        assert_eq!(
            Shell::parse_quoted_command_text("/say \"\"", "/say"),
            None
        );
        assert_eq!(
            Shell::parse_quoted_command_text("/say hello world", "/say"),
            None
        );
        assert_eq!(
            Shell::parse_quoted_command_text("/run \"unterminated", "/run"),
            None
        );
    }

    #[test]
    fn help_and_slash_catalog_include_say_and_run() {
        assert!(HELP_SPEECH.iter().any(|(command, description, _)| {
            *command == "/say \"text\""
                && *description == "speak text verbatim through TTS (audio test)"
        }));
        assert!(HELP_SPEECH.iter().any(|(command, description, _)| {
            *command == "/run \"text\""
                && *description == "run full inference pipeline as if spoken"
        }));
        assert!(SLASH_COMMANDS.iter().any(|command| command.command == "/say"));
        assert!(SLASH_COMMANDS.iter().any(|command| command.command == "/run"));
    }

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
