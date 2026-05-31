use crate::commands::{
    self, CommandArgumentKind, HelpGroup, COMMANDS, HELP_LEFT_COLUMN_GROUPS,
    HELP_RIGHT_COLUMN_GROUPS,
};
use crate::config_editor::ConfigEditor;
use crate::daemon_client::launch_cli_tab;
use crate::error::CliError;
use crate::tab_chrome;
use crate::theme;
use crate::tabs::CliTabKind;
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
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::{Value, json};
use sri::{
    HealthStatus, RegisteredSriNode, ResourceKind, SignalSource, SriEvent, SriResourceSnapshot,
    SriSnapshot, SriTreeNode, TreeAction,
};
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

const HELP_ESC_RESET_AFTER: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteItem {
    Command(usize),
    FixedArgument {
        command_index: usize,
        argument_index: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutocompleteKind {
    Commands,
    FixedArguments { command_index: usize },
}

#[derive(Clone, Debug)]
struct AutocompleteState {
    kind: AutocompleteKind,
    items: Vec<AutocompleteItem>,
    selected: usize,
    scroll_offset: usize,
    no_matches: bool,
    navigation_engaged: bool,
}

impl AutocompleteState {
    const MAX_VISIBLE_ITEMS: usize = 8;

    fn from_input(input: &str) -> Option<Self> {
        let trimmed = input.trim_start();
        if !trimmed.starts_with('/') {
            return None;
        }

        if let Some((command_index, spec)) = commands::find_command(trimmed) {
            return match spec.argument_kind {
                CommandArgumentKind::FixedList(_) => {
                    Some(Self::fixed_arguments(command_index, "", false))
                }
                CommandArgumentKind::None | CommandArgumentKind::FreeText => None,
            };
        }

        if let Some((command, remainder)) = trimmed.split_once(' ')
            && let Some((command_index, spec)) = commands::find_command(command)
        {
            return match spec.argument_kind {
                CommandArgumentKind::FixedList(_) => {
                    Some(Self::fixed_arguments(command_index, remainder.trim_start(), false))
                }
                CommandArgumentKind::None | CommandArgumentKind::FreeText => None,
            };
        }

        Some(Self::command_matches(trimmed))
    }

    fn command_matches(prefix: &str) -> Self {
        let items = COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, command)| command.command.starts_with(prefix))
            .map(|(index, _)| AutocompleteItem::Command(index))
            .collect::<Vec<_>>();

        Self {
            kind: AutocompleteKind::Commands,
            no_matches: items.is_empty() && !prefix.is_empty() && prefix != "/",
            items,
            selected: 0,
            scroll_offset: 0,
            navigation_engaged: false,
        }
    }

    fn fixed_arguments(command_index: usize, prefix: &str, navigation_engaged: bool) -> Self {
        let items = COMMANDS[command_index]
            .fixed_arguments()
            .unwrap_or_default()
            .iter()
            .enumerate()
            .filter(|(_, argument)| argument.value.starts_with(prefix))
            .map(|(argument_index, _)| AutocompleteItem::FixedArgument {
                command_index,
                argument_index,
            })
            .collect::<Vec<_>>();

        Self {
            kind: AutocompleteKind::FixedArguments { command_index },
            no_matches: items.is_empty() && !prefix.is_empty(),
            items,
            selected: 0,
            scroll_offset: 0,
            navigation_engaged,
        }
    }

    fn next(&mut self) {
        if self.items.is_empty() {
            return;
        }

        self.navigation_engaged = true;
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
        self.sync_scroll();
    }

    fn prev(&mut self) {
        if self.items.is_empty() {
            return;
        }

        self.navigation_engaged = true;
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.sync_scroll();
    }

    fn selected_item(&self) -> Option<AutocompleteItem> {
        self.items.get(self.selected).copied()
    }

    fn visible_items(&self) -> &[AutocompleteItem] {
        let end = (self.scroll_offset + Self::MAX_VISIBLE_ITEMS).min(self.items.len());
        &self.items[self.scroll_offset..end]
    }

    fn title(&self) -> String {
        match self.kind {
            AutocompleteKind::Commands => "Command Helper".to_string(),
            AutocompleteKind::FixedArguments { command_index } => {
                format!("{} options", COMMANDS[command_index].command)
            }
        }
    }

    fn no_matches_label(&self) -> &'static str {
        match self.kind {
            AutocompleteKind::Commands => "No matching commands",
            AutocompleteKind::FixedArguments { .. } => "No matching options",
        }
    }

    fn sync_scroll(&mut self) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + Self::MAX_VISIBLE_ITEMS {
            self.scroll_offset = self.selected + 1 - Self::MAX_VISIBLE_ITEMS;
        }
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn accepts_enter_without_navigation(&self) -> bool {
        match self.kind {
            AutocompleteKind::Commands => self.items.len() == 1,
            AutocompleteKind::FixedArguments { .. } => !self.items.is_empty(),
        }
    }

    fn should_apply_on_enter(&self) -> bool {
        self.navigation_engaged || self.accepts_enter_without_navigation()
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

#[derive(Clone, Debug, Default)]
struct HelpOverlayState {
    esc_deadline: Option<Instant>,
}

impl HelpOverlayState {
    fn handle_escape(&mut self, now: Instant) -> bool {
        if self.esc_deadline.is_some_and(|deadline| now <= deadline) {
            self.esc_deadline = None;
            true
        } else {
            self.esc_deadline = Some(now + HELP_ESC_RESET_AFTER);
            false
        }
    }

    fn confirmation_visible(&self, now: Instant) -> bool {
        self.esc_deadline.is_some_and(|deadline| now <= deadline)
    }
}

#[derive(Clone, Debug)]
enum ModalState {
    Models(ModelModal),
    Help(HelpOverlayState),
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
    autocomplete: Option<&'a AutocompleteState>,
    modal: Option<&'a ModalState>,
    close_confirmation_active: bool,
    close_confirmation_remaining: u8,
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
    close_confirmation: tab_chrome::CloseConfirmation,
    full_tree: bool,
    autocomplete: Option<AutocompleteState>,
    modal: Option<ModalState>,
}

impl Shell {
    pub async fn new(mut ipc: IpcClient) -> Result<Self, CliError> {
        enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        tab_chrome::prime_terminal(&mut terminal)?;

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
                loops_map.insert(name.clone(), LoopInfo { enabled });
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
                if tab_chrome::is_shutdown_event(&event) {
                    connection_alive_task.store(false, Ordering::SeqCst);
                    break;
                }

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
                            let maybe_signal = if let (Ok(mut panel), Ok(mut response_log)) =
                                (push_sri_panel.lock(), push_response_log.lock())
                            {
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
            close_confirmation: tab_chrome::CloseConfirmation::new(),
            full_tree: false,
            autocomplete: None,
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
                    autocomplete: self.autocomplete.as_ref(),
                    modal: self.modal.as_ref(),
                    close_confirmation_active: self.close_confirmation.is_active(),
                    close_confirmation_remaining: self.close_confirmation.remaining_seconds(),
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
                        autocomplete: self.autocomplete.as_ref(),
                        modal: self.modal.as_ref(),
                        close_confirmation_active: self.close_confirmation.is_active(),
                        close_confirmation_remaining: self.close_confirmation.remaining_seconds(),
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
        match self.close_confirmation.handle_key(code, modifiers) {
            tab_chrome::CloseAction::Confirmed => {
                self.should_quit = true;
                return Ok(());
            }
            tab_chrome::CloseAction::Armed | tab_chrome::CloseAction::Cancelled => {
                return Ok(());
            }
            tab_chrome::CloseAction::Ignored => {}
        }

        if self.modal.is_some() {
            return self.handle_modal_key_event(code).await;
        }

        if self
            .autocomplete
            .as_ref()
            .is_some_and(|autocomplete| !autocomplete.is_empty() || autocomplete.no_matches)
        {
            match code {
                KeyCode::Up => {
                    if let Some(autocomplete) = &mut self.autocomplete {
                        autocomplete.prev();
                    }
                    return Ok(());
                }
                KeyCode::Down => {
                    if let Some(autocomplete) = &mut self.autocomplete {
                        autocomplete.next();
                    }
                    return Ok(());
                }
                KeyCode::Enter
                    if self
                        .autocomplete
                        .as_ref()
                        .is_some_and(AutocompleteState::should_apply_on_enter) =>
                {
                    self.apply_autocomplete_selection().await?;
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.autocomplete = None;
                    return Ok(());
                }
                _ => {}
            }
        }

        match code {
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
                self.refresh_autocomplete();
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
                self.refresh_autocomplete();
            }
            KeyCode::Enter => {
                self.submit_input_buffer().await?;
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
        if let Some(ModalState::Help(help)) = self.modal.as_mut() {
            if matches!(code, KeyCode::Esc) && help.handle_escape(Instant::now()) {
                self.modal = None;
            }

            return Ok(());
        }

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

        if Self::is_help_command(input) {
            self.handle_slash_command(input).await?;
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

    fn refresh_autocomplete(&mut self) {
        self.autocomplete = AutocompleteState::from_input(&self.input_buffer);
    }

    async fn submit_input_buffer(&mut self) -> Result<(), CliError> {
        let input = std::mem::take(&mut self.input_buffer);
        self.autocomplete = None;
        self.log_scroll = 0;
        self.handle_input(input).await
    }

    async fn apply_autocomplete_selection(&mut self) -> Result<(), CliError> {
        let Some(selected_item) = self
            .autocomplete
            .as_ref()
            .and_then(|autocomplete| autocomplete.selected_item())
        else {
            return Ok(());
        };

        match selected_item {
            AutocompleteItem::Command(command_index) => {
                let command = COMMANDS[command_index];
                self.input_buffer.clear();
                self.input_buffer.push_str(command.command);

                match command.argument_kind {
                    CommandArgumentKind::None => {
                        self.autocomplete = None;
                        self.submit_input_buffer().await?;
                    }
                    CommandArgumentKind::FreeText => {
                        self.input_buffer.push(' ');
                        self.autocomplete = None;
                    }
                    CommandArgumentKind::FixedList(_) => {
                        self.input_buffer.push(' ');
                        self.autocomplete = Some(AutocompleteState::fixed_arguments(
                            command_index,
                            "",
                            true,
                        ));
                    }
                }
            }
            AutocompleteItem::FixedArgument {
                command_index,
                argument_index,
            } => {
                let argument = COMMANDS[command_index]
                    .fixed_arguments()
                    .and_then(|arguments| arguments.get(argument_index))
                    .copied();

                if let Some(argument) = argument {
                    self.input_buffer =
                        format!("{} {}", COMMANDS[command_index].command, argument.value);
                }
                self.autocomplete = None;
            }
        }

        Ok(())
    }

    fn sync_uptime(&mut self, uptime_secs: u64) {
        self.daemon_uptime_secs = uptime_secs;
        self.daemon_uptime_anchor = Instant::now();
    }

    fn current_uptime_secs(&self) -> u64 {
        self.daemon_uptime_secs + self.daemon_uptime_anchor.elapsed().as_secs()
    }

    fn parse_command_text(input: &str, command: &str) -> Option<String> {
        let remainder = input.trim().strip_prefix(command)?.trim();
        if remainder.is_empty() {
            return None;
        }

        let text = if let Some(quoted) = remainder.strip_prefix('"').and_then(|text| text.strip_suffix('"')) {
            quoted
        } else {
            remainder
        };

        if text.trim().is_empty() {
            None
        } else {
            Some(text.to_string())
        }
    }

    fn is_help_command(input: &str) -> bool {
        matches!(input.trim(), "/help" | "/?")
    }

    async fn handle_slash_command(&mut self, input: &str) -> Result<(), CliError> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        if matches!(input.trim(), "/memory clear" | "/mem clear") {
            self.cmd_memory_clear().await?;
            return Ok(());
        }

        match parts[0] {
            "/help" | "/?" => self.cmd_help().await?,
            "/quit" | "/exit" | "/bye" => {
                self.should_quit = true;
            }
            "/tab" => self.cmd_tab(parts.get(1).copied()).await?,
            "/status" | "/health" => self.cmd_status().await?,
            "/ping" | "/uptime" => self.cmd_ping().await?,
            "/shutdown" => self.cmd_shutdown().await?,
            "/models" => self.cmd_open_model_modal().await?,
            "/model" => match parts.get(1).copied() {
                Some("load") => {
                    let path = Self::parse_command_text(input, "/model load");
                    self.cmd_load_model(path.as_deref()).await?
                }
                _ => self.cmd_open_model_modal().await?,
            },
            "/load" => {
                let path = Self::parse_command_text(input, "/load");
                self.cmd_load_model(path.as_deref()).await?
            }
            "/listen" | "/mic" => self.cmd_listen_start().await?,
            "/stop" | "/end" => self.cmd_listen_stop().await?,
            "/say" => self.cmd_say(input).await?,
            "/run" => self.cmd_run(input).await?,
            "/observation" | "/obs" => self.cmd_observation().await?,
            "/memory" | "/mem" => self.cmd_transparency_memory().await?,
            "/memory-stats" | "/memstats" => self.cmd_memory_stats().await?,
            "/memory-clear" | "/memclear" => self.cmd_memory_clear().await?,
            "/debug" => self.cmd_debug(parts.get(1).copied()),
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
        self.modal = Some(ModalState::Help(HelpOverlayState::default()));
        Ok(())
    }

    async fn cmd_tab(&mut self, name: Option<&str>) -> Result<(), CliError> {
        let Some(name) = name else {
            self.log_message("Usage: /tab <diag|config|actors|resources>".to_string());
            return Ok(());
        };

        if commands::find_fixed_argument("/tab", name).is_none() {
            self.log_message(format!(
                "Unknown tab '{}'. Use diag, config, actors, or resources.",
                name
            ));
            return Ok(());
        }

        let Some(tab) = CliTabKind::parse(name) else {
            self.log_message(format!(
                "Unknown tab '{}'. Use diag, config, actors, or resources.",
                name
            ));
            return Ok(());
        };

        match launch_cli_tab(tab) {
            Ok(()) => self.log_message(format!("Opening '{}' tab window...", name)),
            Err(error) => self.log_message(format!("Could not open '{}' tab: {}", name, error)),
        }
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
        let Some(text) = Self::parse_command_text(input, "/say") else {
            self.log_message("usage: /say <text to speak>".to_string());
            return Ok(());
        };

        match self.ipc.send("speech.say", json!({"text": text})).await {
            Ok(_) => self.log_message(format!("[SAY] \"{}\"", text)),
            Err(e) => self.log_message(format!("Could not send speech.say: {}", e)),
        }

        Ok(())
    }

    async fn cmd_run(&mut self, input: &str) -> Result<(), CliError> {
        let Some(text) = Self::parse_command_text(input, "/run") else {
            self.log_message("usage: /run <text to process>".to_string());
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

    async fn cmd_memory_clear(&mut self) -> Result<(), CliError> {
        match self.ipc.send("memory.clear", json!({})).await {
            Ok(_) => self.log_message("[MEM] memory cleared".to_string()),
            Err(e) => self.log_message(format!("Could not clear memory: {}", e)),
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

    fn cmd_debug(&mut self, subsystem: Option<&str>) {
        let Some(subsystem) = subsystem else {
            self.log_message(
                "Usage: /debug <inference|speech|memory|ctp|soul|platform|sri>".to_string(),
            );
            return;
        };

        if commands::find_fixed_argument("/debug", subsystem).is_none() {
            self.log_message(format!(
                "Unknown debug target '{}'. Use inference, speech, memory, ctp, soul, platform, or sri.",
                subsystem
            ));
            return;
        }

        self.log_message(format!(
            "Debug tracing hint set to '{}'. Runtime log-level hot swap is not wired yet.",
            subsystem
        ));
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
                                loops_map.insert(name.clone(), LoopInfo { enabled });
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
            ModalState::Help(_) => None,
        });

        let Some(model) = model else {
            self.modal = None;
            return Ok(());
        };

        self.modal = None;
        self.log_message(format!("Loading model '{}'...", model.name));
        self.cmd_load_model(Some(model.path.as_str())).await
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
                    panel.active_shelf =
                        Self::select_active_shelf(snapshot, panel.active_shelf.as_deref());
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
                "[FAULT] {} {} {} > {}",
                actor,
                Self::resource_label(resource),
                Self::format_resource_alert_value(resource, value),
                Self::format_resource_alert_value(resource, threshold)
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
            nodes
                .iter()
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
        tab_chrome::sync_terminal_before_draw(terminal)?;
        terminal.draw(|frame| {
            if render.close_confirmation_active {
                tab_chrome::render_close_confirmation(frame, render.close_confirmation_remaining);
                return;
            }

            let vertical = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Min(0),
                    Constraint::Length(6),
                    Constraint::Length(3),
                ])
                .split(frame.area());

            tab_chrome::render_header(
                frame,
                vertical[0],
                "LIVE",
                render.daemon_status,
                render.daemon_uptime_secs,
                Some(tab_chrome::close_hint()),
            );

            let top = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(31),
                    Constraint::Percentage(37),
                    Constraint::Percentage(32),
                ])
                .split(vertical[1]);

            Self::render_tree_panel(frame, top[0], render.sri_panel, render.full_tree);
            Self::render_signal_panel(frame, top[1], render.message_log, render.log_scroll);
            Self::render_response_panel(frame, top[2], render.response_log);
            Self::render_resources_panel(
                frame,
                vertical[2],
                render.sri_panel,
                render.daemon_status,
                render.daemon_uptime_secs,
            );
            Self::render_input(
                frame,
                vertical[3],
                render.input_buffer,
                render.daemon_status,
                render.daemon_uptime_secs,
            );

            if render.modal.is_none() {
                Self::render_autocomplete(frame, vertical[3], render.autocomplete);
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
        let title = if full_tree {
            "Capability Tree [ALL]"
        } else {
            "Capability Tree [LIVE]"
        };
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
            Span::styled(
                format!("{}{}{} ", prefix, connector, marker),
                theme::muted(),
            ),
            Span::styled(
                node.display_name.clone(),
                if is_active {
                    theme::title_style()
                } else {
                    theme::text()
                },
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
            "CPU",
            snapshot.total_cpu_pct.clamp(0.0, 100.0),
            format!("{:.1}%", snapshot.total_cpu_pct),
            Self::cpu_resource_style(snapshot.total_cpu_pct),
        )];

        if let (Some(used_mb), Some(total_mb)) = (snapshot.vram_used_mb, snapshot.vram_total_mb) {
            let percent = if total_mb == 0 {
                0.0
            } else {
                used_mb as f32 / total_mb as f32 * 100.0
            };
            lines.push(Self::resource_line(
                "VRAM",
                percent.clamp(0.0, 100.0),
                format!(
                    "{:.2} GB / {:.2} GB",
                    used_mb as f32 / 1024.0,
                    total_mb as f32 / 1024.0
                ),
                Self::vram_resource_style(percent),
            ));
        }

        lines
    }

    fn resource_line(label: &str, percent: f32, total: String, style: Style) -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{:<4}", label), theme::muted()),
            Span::styled(
                format!("[{}] {}", Self::resource_bar(percent, 10), total),
                style,
            ),
        ])
    }

    fn resource_bar(percent: f32, width: usize) -> String {
        let clamped = percent.clamp(0.0, 100.0);
        let filled = ((clamped / 100.0) * width as f32).round() as usize;
        let empty = width.saturating_sub(filled.min(width));
        format!("{}{}", "█".repeat(filled.min(width)), "░".repeat(empty))
    }

    fn cpu_resource_style(percent: f32) -> Style {
        if percent < 40.0 {
            theme::success()
        } else if percent <= 70.0 {
            theme::warning()
        } else {
            theme::danger()
        }
    }

    fn vram_resource_style(percent: f32) -> Style {
        if percent < 70.0 {
            theme::success()
        } else if percent <= 90.0 {
            theme::warning()
        } else {
            theme::danger()
        }
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

    fn format_resource_alert_value(resource: ResourceKind, value: f32) -> String {
        match resource {
            ResourceKind::Ram => format!("{:.2} GB", value / 1024.0),
            ResourceKind::Cpu | ResourceKind::Vram => format!("{:.1}%", value),
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

    fn render_autocomplete(
        frame: &mut Frame,
        input_area: Rect,
        autocomplete: Option<&AutocompleteState>,
    ) {
        let Some(autocomplete) = autocomplete else {
            return;
        };

        if autocomplete.no_matches {
            let panel_title = autocomplete.title();
            let popup_area = Rect {
                x: input_area.x + 1,
                y: input_area.y.saturating_sub(3),
                width: 40u16.min(frame.area().width.saturating_sub(2)),
                height: 3,
            };
            frame.render_widget(Clear, popup_area);
            let panel = Paragraph::new(Line::from(Span::styled(
                autocomplete.no_matches_label(),
                theme::muted(),
            )))
            .block(theme::panel(&panel_title));
            frame.render_widget(panel, popup_area);
            return;
        }

        if autocomplete.is_empty() {
            return;
        }

        let visible_items = autocomplete.visible_items();
        let visible_count = visible_items.len();
        let popup_area = Rect {
            x: input_area.x + 1,
            y: input_area.y.saturating_sub((visible_count + 2) as u16),
            width: 64u16.min(frame.area().width.saturating_sub(2)),
            height: (visible_count + 2) as u16,
        };
        frame.render_widget(Clear, popup_area);

        let items = visible_items
            .iter()
            .map(|item| {
                let (label, description) = match item {
                    AutocompleteItem::Command(index) => {
                        let command = &COMMANDS[*index];
                        (command.command, command.description)
                    }
                    AutocompleteItem::FixedArgument {
                        command_index,
                        argument_index,
                    } => {
                        let argument = COMMANDS[*command_index]
                            .fixed_arguments()
                            .and_then(|arguments| arguments.get(*argument_index))
                            .copied()
                            .expect("fixed argument should exist");
                        (argument.value, argument.description)
                    }
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!("{: <12}", label), theme::title_style()),
                    Span::styled("  ", theme::text()),
                    Span::styled(description, theme::text()),
                ]))
            })
            .collect::<Vec<_>>();

        let mut state = ListState::default();
        state.select(Some(
            autocomplete
                .selected
                .saturating_sub(autocomplete.scroll_offset)
                .min(visible_count.saturating_sub(1)),
        ));
        let panel_title = autocomplete.title();
        let list = List::new(items)
            .block(theme::focused_panel(&panel_title))
            .highlight_style(theme::selected());
        frame.render_stateful_widget(list, popup_area, &mut state);
    }

    fn render_modal(frame: &mut Frame, modal: &ModalState) {
        match modal {
            ModalState::Help(help_overlay) => Self::render_help_overlay(frame, help_overlay),
            ModalState::Models(model_modal) => Self::render_model_modal(frame, model_modal),
        }
    }

    fn render_help_overlay(frame: &mut Frame, help_overlay: &HelpOverlayState) {
        let area = frame.area();
        frame.render_widget(Clear, area);

        let block = theme::overlay_panel("Command Guide");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(0),
                Constraint::Length(2),
            ])
            .split(inner);

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                "Sena Manual Controls",
                theme::overlay_text().add_modifier(Modifier::BOLD),
            ),
            Span::styled("  Full-screen command reference", theme::overlay_muted()),
        ]))
        .style(theme::overlay_text());
        frame.render_widget(title, sections[0]);

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(sections[1]);

        let left = Paragraph::new(Self::help_overlay_column_lines(HELP_LEFT_COLUMN_GROUPS))
        .style(theme::overlay_text())
        .wrap(Wrap { trim: false });
        frame.render_widget(left, columns[0]);

        let right = Paragraph::new(Self::help_overlay_column_lines(HELP_RIGHT_COLUMN_GROUPS))
        .style(theme::overlay_text())
        .wrap(Wrap { trim: false });
        frame.render_widget(right, columns[1]);

        let footer_text = if help_overlay.confirmation_visible(Instant::now()) {
            "Press Esc again to return to Sena"
        } else {
            "Press Esc twice to return  ·  Esc once cancels any pending input"
        };
        let footer_style = if help_overlay.confirmation_visible(Instant::now()) {
            theme::overlay_muted().add_modifier(Modifier::DIM)
        } else {
            theme::overlay_muted()
        };
        let footer = Paragraph::new(Line::from(Span::styled(footer_text, footer_style)))
            .style(theme::overlay_text());
        frame.render_widget(footer, sections[2]);
    }

    fn help_overlay_column_lines(groups: &[HelpGroup]) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        for (column_index, group) in groups.iter().enumerate() {
            if column_index > 0 {
                lines.push(Line::from(String::new()));
            }

            lines.push(Line::from(Span::styled(
                group.title(),
                theme::overlay_text()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            )));

            for command in commands::commands_in_group(*group) {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:<22}", command.command),
                        theme::overlay_text().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(command.description, theme::overlay_text()),
                ]));
            }
        }

        lines
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
    use super::{AutocompleteItem, AutocompleteState, HelpOverlayState, Shell};
    use crate::commands::{CommandArgumentKind, COMMANDS, TAB_ARGUMENTS, find_command};
    use serde_json::json;
    use sri::SriResourceSnapshot;
    use std::time::{Duration, Instant};

    #[test]
    fn command_text_parses_balanced_quotes_and_free_text() {
        assert_eq!(
            Shell::parse_command_text("/say \"hello world\"", "/say"),
            Some("hello world".to_string())
        );
        assert_eq!(
            Shell::parse_command_text("/run \"  hi there  \"", "/run"),
            Some("  hi there  ".to_string())
        );
        assert_eq!(
            Shell::parse_command_text("/run hello world", "/run"),
            Some("hello world".to_string())
        );
        assert_eq!(
            Shell::parse_command_text("/load C:/models/sena.gguf", "/load"),
            Some("C:/models/sena.gguf".to_string())
        );

        assert_eq!(Shell::parse_command_text("/say", "/say"), None);
        assert_eq!(Shell::parse_command_text("/say \"\"", "/say"), None);
        assert_eq!(
            Shell::parse_command_text("/run \"unterminated", "/run"),
            Some("\"unterminated".to_string())
        );
    }

    #[test]
    fn command_registry_includes_say_run_and_tab() {
        let (_, say) = find_command("/say").expect("/say should be registered");
        let (_, run) = find_command("/run").expect("/run should be registered");
        let (_, tab) = find_command("/tab").expect("/tab should be registered");
        let (_, memory_clear) =
            find_command("/memory clear").expect("/memory clear should be registered");

        assert_eq!(say.description, "Speak text verbatim through TTS (audio test)");
        assert_eq!(run.description, "Run full inference pipeline as if spoken");
        assert_eq!(tab.argument_kind, CommandArgumentKind::FixedList(TAB_ARGUMENTS));
        assert_eq!(
            memory_clear.description,
            "Clear persistent memory contents"
        );
        assert!(COMMANDS.iter().any(|command| command.command == "/debug"));
    }

    #[test]
    fn autocomplete_filters_commands_without_arming_enter_selection() {
        let autocomplete = AutocompleteState::from_input("/t").expect("autocomplete should open");
        let commands = autocomplete
            .items
            .iter()
            .filter_map(|item| match item {
                AutocompleteItem::Command(index) => Some(COMMANDS[*index].command),
                AutocompleteItem::FixedArgument { .. } => None,
            })
            .collect::<Vec<_>>();

        assert!(!autocomplete.navigation_engaged);
        assert!(autocomplete.visible_items().len() >= 2);
        assert_eq!(autocomplete.selected_item(), Some(AutocompleteItem::Command(0)));
        assert_eq!(commands.first().copied(), Some("/tab"));
        assert!(commands.contains(&"/tree"));
        assert!(!commands.contains(&"/test-mode"));
    }

    #[test]
    fn autocomplete_opens_fixed_argument_dropdown_for_exact_tab_command() {
        let autocomplete = AutocompleteState::from_input("/tab").expect("tab args should open");

        let values = autocomplete
            .items
            .iter()
            .map(|item| match item {
                AutocompleteItem::FixedArgument {
                    command_index,
                    argument_index,
                } => COMMANDS[*command_index]
                    .fixed_arguments()
                    .and_then(|arguments| arguments.get(*argument_index))
                    .expect("argument should exist")
                    .value,
                AutocompleteItem::Command(_) => "",
            })
            .collect::<Vec<_>>();

        assert_eq!(values, vec!["diag", "config", "actors", "resources"]);
        assert!(autocomplete.accepts_enter_without_navigation());
    }

    #[test]
    fn autocomplete_accepts_implicit_enter_for_single_command_match() {
        let autocomplete =
            AutocompleteState::from_input("/ta").expect("autocomplete should open for /ta");

        assert_eq!(autocomplete.items.len(), 1);
        assert_eq!(autocomplete.selected_item(), Some(AutocompleteItem::Command(0)));
        assert!(autocomplete.accepts_enter_without_navigation());
    }

    #[test]
    fn free_text_commands_skip_argument_dropdown() {
        assert!(AutocompleteState::from_input("/run hello there").is_none());
        assert!(AutocompleteState::from_input("/say hello there").is_none());
        assert!(AutocompleteState::from_input("/query recent changes").is_none());
    }

    #[test]
    fn autocomplete_scrolls_when_more_than_eight_items_are_visible() {
        let mut autocomplete = AutocompleteState::from_input("/").expect("autocomplete should open");

        assert_eq!(autocomplete.visible_items().len(), AutocompleteState::MAX_VISIBLE_ITEMS);
        for _ in 0..AutocompleteState::MAX_VISIBLE_ITEMS {
            autocomplete.next();
        }

        assert_eq!(autocomplete.scroll_offset, 1);
    }

    #[test]
    fn help_overlay_requires_double_escape_within_window() {
        let start = Instant::now();
        let mut help = HelpOverlayState::default();

        assert!(!help.handle_escape(start));
        assert!(help.confirmation_visible(start + Duration::from_secs(1)));
        assert!(help.handle_escape(start + Duration::from_secs(1)));
        assert!(!help.confirmation_visible(start + Duration::from_secs(1)));
    }

    #[test]
    fn help_overlay_escape_confirmation_expires_after_window() {
        let start = Instant::now();
        let mut help = HelpOverlayState::default();

        assert!(!help.handle_escape(start));
        assert!(!help.confirmation_visible(start + Duration::from_secs(3)));
        assert!(!help.handle_escape(start + Duration::from_secs(3)));
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
    fn resource_lines_omit_ram_and_keep_cpu_and_vram() {
        let snapshot = SriResourceSnapshot {
            total_ram_mb: 8 * 1024,
            total_cpu_pct: 37.5,
            vram_used_mb: Some(3 * 1024),
            vram_total_mb: Some(8 * 1024),
        };

        let lines = Shell::resource_lines(&snapshot);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(rendered.len(), 2);
        assert!(rendered[0].contains("CPU"));
        assert!(rendered[1].contains("VRAM"));
        assert!(rendered.iter().all(|line| !line.trim_start().starts_with("RAM")));
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
