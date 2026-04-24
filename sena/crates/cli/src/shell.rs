use crate::config_editor::ConfigEditor;
use crate::error::CliError;
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
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
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

pub struct Shell {
    ipc: IpcClient,
    message_log: Arc<Mutex<Vec<String>>>,
    loops: Arc<Mutex<HashMap<String, LoopInfo>>>,
    vram: Arc<Mutex<Option<VramState>>>,
    input_buffer: String,
    should_quit: bool,
    daemon_status: String,
    daemon_uptime_secs: u64,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    connection_alive: Arc<AtomicBool>,
    log_scroll: usize,
    quit_armed: bool,
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
                    let total_mb = data.get("total_mb").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
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

                if let Some(line) = Self::format_push_event(&event) {
                    if let Ok(mut log) = push_log.lock() {
                        log.push(line);
                        if log.len() > 500 {
                            log.drain(0..100);
                        }
                    }
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
            terminal,
            connection_alive,
            log_scroll: 0,
            quit_armed: false,
        })
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
            "InferenceSentenceReady" => {
                let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
                Some(format!("[INF] {}", text))
            }
            "InferenceStreamCompleted" => {
                let token_count = data.get("token_count").and_then(|v| v.as_u64()).unwrap_or(0);
                Some(format!("[INF] ✓ stream done ({} tokens)", token_count))
            }
            "SpeakingStarted" => Some("[TTS] speaking started".to_string()),
            "SpeakingCompleted" => Some("[TTS] done".to_string()),
            "MemoryWriteCompleted" => Some("[MEM] write ok".to_string()),
            "MemoryWriteFailed" => {
                let reason = data.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown");
                Some(format!("[MEM] failed: {}", reason))
            }
            "ActorFailed" => {
                let actor = data.get("actor").and_then(|v| v.as_str()).unwrap_or("?");
                let reason = data.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown");
                Some(format!("[ERR] {}: {}", actor, reason))
            }
            "ThoughtEventTriggered" => {
                let app = data.get("app").and_then(|v| v.as_str()).unwrap_or("?");
                let task = data.get("task").and_then(|v| v.as_str()).unwrap_or("unknown");
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
                let enabled = data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
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
            Self::render_tui(
                &mut self.terminal,
                &log,
                &loops_vec,
                &self.input_buffer,
                &self.daemon_status,
                self.daemon_uptime_secs,
                self.log_scroll,
                self.vram.lock().ok().and_then(|v| *v),
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
                Self::render_tui(
                    &mut self.terminal,
                    &log,
                    &loops_vec,
                    &self.input_buffer,
                    &self.daemon_status,
                    self.daemon_uptime_secs,
                    self.log_scroll,
                    self.vram.lock().ok().and_then(|v| *v),
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
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Enter => {
                let input = self.input_buffer.clone();
                self.input_buffer.clear();
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

    async fn handle_input(&mut self, input: String) -> Result<(), CliError> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }

        self.log_message(format!("> {}", input));

        if input.starts_with('/') {
            self.handle_slash_command(input).await?;
        } else {
            self.log_message("Sena listens by voice. Type / for commands.".to_string());
        }

        Ok(())
    }

    async fn handle_slash_command(&mut self, input: &str) -> Result<(), CliError> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        match parts[0] {
            "/help" => self.cmd_help().await?,
            "/quit" | "/exit" => {
                self.should_quit = true;
            }
            "/status" => self.cmd_status().await?,
            "/ping" => self.cmd_ping().await?,
            "/shutdown" => self.cmd_shutdown().await?,
            "/models" => self.cmd_list_models().await?,
            "/load" => self.cmd_load_model(parts.get(1).copied()).await?,
            "/listen" => self.cmd_listen_start().await?,
            "/stop" => self.cmd_listen_stop().await?,
            "/memory" => self.cmd_memory_stats().await?,
            "/query" => self.cmd_memory_query(&parts[1..]).await?,
            "/config" => self.open_config_editor().await?,
            "/events" => self.cmd_events_subscribe().await?,
            "/inference" => self.cmd_inference_status().await?,
            "/speech" => self.cmd_speech_status().await?,
            "/loops" => match parts.get(1).copied() {
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
                self.log_message(format!("Unknown command: {}", other));
            }
        }

        Ok(())
    }

    async fn cmd_help(&mut self) -> Result<(), CliError> {
        match self.ipc.send("list_commands", json!({})).await {
            Ok(response) => {
                self.log_message("Available commands:".to_string());
                if let Some(commands) = response.get("commands").and_then(|v| v.as_array()) {
                    let mut rows: Vec<(String, String)> = commands
                        .iter()
                        .map(|cmd| {
                            (
                                cmd.get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("?")
                                    .to_string(),
                                cmd.get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            )
                        })
                        .collect();
                    rows.sort_by(|a, b| a.0.cmp(&b.0));
                    for (name, description) in rows {
                        self.log_message(format!("  {} - {}", name, description));
                    }
                } else {
                    self.log_message(format!("  {}", response));
                }
            }
            Err(e) => {
                self.log_message(format!("/help failed: {}", e));
            }
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
                self.log_message(format!("Status: {} | Uptime: {}s", status, uptime));

                if let Some(actors) = response.get("actors").and_then(|v| v.as_array()) {
                    for actor in actors {
                        let name = actor.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let status_text = actor
                            .get("status")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| actor.get("status").map_or("?".to_string(), |v| v.to_string()));
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
                    self.daemon_uptime_secs = uptime;
                    self.log_message(format!("Pong: uptime {}s", uptime));
                } else {
                    self.log_message(format!("Pong: {}", response));
                }
            }
            Err(e) => {
                self.log_message(format!("Ping failed: {}", e));
            }
        }
        Ok(())
    }

    async fn cmd_shutdown(&mut self) -> Result<(), CliError> {
        match self.ipc.send("runtime.shutdown", json!({})).await {
            Ok(_) => {
                self.log_message("Shutdown initiated. Daemon will disconnect.".to_string());
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

    async fn cmd_list_models(&mut self) -> Result<(), CliError> {
        match self.ipc.send("inference.list_models", json!({})).await {
            Ok(response) => self.log_message(format!("Models: {}", response)),
            Err(e) => self.log_message(format!("List models failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_load_model(&mut self, path: Option<&str>) -> Result<(), CliError> {
        let Some(path) = path else {
            self.log_message("Usage: /load <path>".to_string());
            return Ok(());
        };

        match self
            .ipc
            .send("inference.load_model", json!({"path": path}))
            .await
        {
            Ok(response) => self.log_message(format!("Load model: {}", response)),
            Err(e) => self.log_message(format!("Load model failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_listen_start(&mut self) -> Result<(), CliError> {
        match self.ipc.send("speech.listen_start", json!({})).await {
            Ok(response) => self.log_message(format!("Listen start: {}", response)),
            Err(e) => self.log_message(format!("Listen start failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_listen_stop(&mut self) -> Result<(), CliError> {
        match self.ipc.send("speech.listen_stop", json!({})).await {
            Ok(response) => self.log_message(format!("Listen stop: {}", response)),
            Err(e) => self.log_message(format!("Listen stop failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_memory_stats(&mut self) -> Result<(), CliError> {
        match self.ipc.send("memory.stats", json!({})).await {
            Ok(response) => self.log_message(format!("Memory stats: {}", response)),
            Err(e) => self.log_message(format!("Memory stats failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_memory_query(&mut self, terms: &[&str]) -> Result<(), CliError> {
        if terms.is_empty() {
            self.log_message("Usage: /query <text>".to_string());
            return Ok(());
        }
        let query = terms.join(" ");
        match self
            .ipc
            .send("memory.query", json!({"query": query}))
            .await
        {
            Ok(response) => self.log_message(format!("Memory query: {}", response)),
            Err(e) => self.log_message(format!("Memory query failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_events_subscribe(&mut self) -> Result<(), CliError> {
        match self.ipc.send("events.subscribe", json!({})).await {
            Ok(response) => self.log_message(format!("Event stream: {}", response)),
            Err(e) => self.log_message(format!("events.subscribe failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_inference_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("inference.status", json!({})).await {
            Ok(response) => self.log_message(format!("Inference status: {}", response)),
            Err(e) => self.log_message(format!("Inference status failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_speech_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("speech.status", json!({})).await {
            Ok(response) => self.log_message(format!("Speech status: {}", response)),
            Err(e) => self.log_message(format!("Speech status failed: {}", e)),
        }
        Ok(())
    }

    async fn cmd_loops(&mut self, name: Option<&str>, state: Option<&str>) -> Result<(), CliError> {
        match (name, state) {
            (None, None) => {
                match self.ipc.send("loops.list", json!({})).await {
                    Ok(response) => {
                        self.log_message("Background Loops:".to_string());
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
                    Err(e) => self.log_message(format!("Loops list failed: {}", e)),
                }
            }
            (Some(name), Some("on")) => {
                match self
                    .ipc
                    .send("loops.set", json!({"loop_name": name, "enabled": true}))
                    .await
                {
                    Ok(response) => self.log_message(format!("Loop {} enabled: {}", name, response)),
                    Err(e) => self.log_message(format!("Loop enable failed: {}", e)),
                }
            }
            (Some(name), Some("off")) => {
                match self
                    .ipc
                    .send("loops.set", json!({"loop_name": name, "enabled": false}))
                    .await
                {
                    Ok(response) => self.log_message(format!("Loop {} disabled: {}", name, response)),
                    Err(e) => self.log_message(format!("Loop disable failed: {}", e)),
                }
            }
            (Some(name), None) => {
                self.log_message(format!("Toggle syntax: /loops {} on|off", name));
            }
            (_, Some(invalid)) => {
                self.log_message(format!("Invalid state '{}'. Use on|off.", invalid));
            }
        }
        Ok(())
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
        message_log: &[String],
        loops: &[LoopInfo],
        input_buffer: &str,
        daemon_status: &str,
        daemon_uptime_secs: u64,
        log_scroll: usize,
        vram: Option<VramState>,
    ) -> Result<(), io::Error> {
        terminal.draw(|frame| {
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(0),
                    Constraint::Length(30),
                ])
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
                daemon_status,
                daemon_uptime_secs,
                vram,
            );
            Self::render_message_log(frame, left_chunks[1], message_log, log_scroll);
            Self::render_input(frame, left_chunks[2], input_buffer);
            Self::render_loops_sidebar(frame, main_chunks[1], loops);
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
            Span::styled(
                "SENA DEV",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  uptime: "),
            Span::styled(
                format!("{}s", daemon_uptime_secs),
                Style::default().fg(Color::White),
            ),
            Span::raw("  daemon: "),
            Span::styled(
                daemon_status,
                if daemon_status == "Connected" {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
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
                Style::default().fg(Color::Green)
            } else if vram_state.percent <= 90 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Red)
            };

            spans.push(Span::raw("  VRAM "));
            spans.push(Span::styled(
                format!("[{}] {:.1} / {:.1} GB", bar, used_gb, total_gb),
                vram_style,
            ));
        }

        let header = Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::ALL).title("Status"));

        frame.render_widget(header, area);
    }

    fn vram_bar(percent: u8) -> String {
        let filled = ((percent.min(100) as usize) * 10 + 50) / 100;
        let empty = 10usize.saturating_sub(filled);
        format!("{}{}", "█".repeat(filled), "░".repeat(empty))
    }

    fn render_message_log(frame: &mut Frame, area: Rect, message_log: &[String], scroll: usize) {
        let height = area.height.saturating_sub(2) as usize;
        let total = message_log.len();
        let end = total.saturating_sub(scroll);
        let start = end.saturating_sub(height);

        let visible_lines: Vec<Line> = message_log[start..end]
            .iter()
            .map(|msg| Line::from(msg.as_str()))
            .collect();

        let title = if scroll > 0 {
            format!("Messages (↑{} lines)", scroll)
        } else {
            "Messages".to_string()
        };

        let para = Paragraph::new(visible_lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false });

        frame.render_widget(para, area);
    }

    fn render_input(frame: &mut Frame, area: Rect, input_buffer: &str) {
        let input = Paragraph::new(input_buffer)
            .block(Block::default().borders(Borders::ALL).title("Input"))
            .style(Style::default().fg(Color::White));

        frame.render_widget(input, area);
    }

    fn render_loops_sidebar(frame: &mut Frame, area: Rect, loops: &[LoopInfo]) {
        let mut sorted = loops.to_vec();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));

        let items: Vec<ListItem> = sorted
            .iter()
            .map(|loop_info| {
                let dot = if loop_info.enabled {
                    Span::styled("● ", Style::default().fg(Color::Green))
                } else {
                    Span::styled("● ", Style::default().fg(Color::Red))
                };
                let text = if loop_info.description.is_empty() {
                    loop_info.name.clone()
                } else {
                    format!("{}", loop_info.name)
                };
                ListItem::new(Line::from(vec![dot, Span::raw(text)]))
            })
            .collect();

        let loops_widget =
            List::new(items).block(Block::default().borders(Borders::ALL).title("Loops"));

        frame.render_widget(loops_widget, area);
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
