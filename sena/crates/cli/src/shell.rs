use crate::config_editor::ConfigEditor;
use crate::error::CliError;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
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
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use serde_json::json;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::time::Duration;
use tracing::info;

/// TUI shell state.
pub struct Shell {
    ipc: IpcClient,
    message_log: Arc<Mutex<Vec<String>>>,
    input_buffer: String,
    should_quit: bool,
    daemon_status: String,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl Shell {
    /// Create and initialize a new shell with IPC connection.
    pub async fn new(ipc: IpcClient) -> Result<Self, CliError> {
        // Setup terminal
        enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let backend = CrosstermBackend::new(stdout);
        let terminal =
            Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        let message_log = Arc::new(Mutex::new(vec![
            "Welcome to Sena CLI".to_string(),
            "Type /help for commands, Ctrl+C to quit".to_string(),
        ]));

        // Spawn task to receive push events
        let push_log = Arc::clone(&message_log);
        let mut push_rx = ipc.subscribe_events();
        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                if let Ok(mut log) = push_log.lock() {
                    log.push(format!("[EVENT] {}", event));
                    // Keep log size reasonable
                    if log.len() > 500 {
                        log.drain(0..100);
                    }
                }
            }
        });

        Ok(Self {
            ipc,
            message_log,
            input_buffer: String::new(),
            should_quit: false,
            daemon_status: "Connected".to_string(),
            terminal,
        })
    }

    /// Run the shell event loop.
    pub async fn run(mut self) -> Result<(), CliError> {
        info!("Shell TUI starting");

        // Initial render
        if let Ok(log) = self.message_log.lock() {
            Self::render_tui(
                &mut self.terminal,
                &log,
                &self.input_buffer,
                &self.daemon_status,
            )
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        }

        while !self.should_quit {
            // Check for terminal events with a short timeout
            if event::poll(Duration::from_millis(100))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && let Event::Key(key) =
                    event::read().map_err(|e| CliError::TuiRenderError(e.to_string()))?
            {
                self.handle_key_event(key.code, key.modifiers).await?;
            }

            // Render
            if let Ok(log) = self.message_log.lock() {
                Self::render_tui(
                    &mut self.terminal,
                    &log,
                    &self.input_buffer,
                    &self.daemon_status,
                )
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
            }
        }

        // Cleanup terminal
        self.cleanup_terminal()?;

        info!("Shell TUI stopped");
        Ok(())
    }

    /// Handle keyboard input.
    async fn handle_key_event(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<(), CliError> {
        match code {
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
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
                self.handle_input(input).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle submitted input line.
    async fn handle_input(&mut self, input: String) -> Result<(), CliError> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(());
        }

        // Echo input
        self.log_message(format!("> {}", input));

        if input.starts_with('/') {
            self.handle_slash_command(input).await?;
        } else {
            self.log_message(
                "Use slash commands (e.g., /help). Voice input requires daemon speech module."
                    .to_string(),
            );
        }

        Ok(())
    }

    /// Handle a slash command.
    async fn handle_slash_command(&mut self, input: &str) -> Result<(), CliError> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        let command = parts[0];
        match command {
            "/help" => self.show_help(),
            "/quit" | "/exit" => {
                self.should_quit = true;
            }
            "/status" => self.cmd_status().await?,
            "/ping" => self.cmd_ping().await?,
            "/shutdown" => self.cmd_shutdown().await?,
            "/models" => self.cmd_list_models().await?,
            "/inference" => self.cmd_inference_status().await?,
            "/memory" => self.cmd_memory_stats().await?,
            "/speech" => self.cmd_speech_status().await?,
            "/config" => {
                self.cmd_config(parts.get(1).copied(), parts.get(2).copied())
                    .await?
            }
            "/events" => self.cmd_events_subscribe().await?,
            _ => {
                self.log_message(format!(
                    "Unknown command: {}. Type /help for available commands.",
                    command
                ));
            }
        }

        Ok(())
    }

    /// Show help text.
    fn show_help(&mut self) {
        self.log_message("Available commands:".to_string());
        self.log_message("  /help       - Show this help".to_string());
        self.log_message("  /status     - Show daemon status".to_string());
        self.log_message("  /ping       - Ping daemon".to_string());
        self.log_message("  /shutdown   - Gracefully shutdown daemon".to_string());
        self.log_message("  /models     - List available models".to_string());
        self.log_message("  /inference  - Show inference status".to_string());
        self.log_message("  /memory     - Show memory stats".to_string());
        self.log_message("  /speech     - Show speech status".to_string());
        self.log_message("  /config [key] [value] - Get or set config".to_string());
        self.log_message("  /events     - Subscribe to daemon events".to_string());
        self.log_message("  /quit       - Exit CLI".to_string());
    }

    /// Execute /status command.
    async fn cmd_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("runtime.status", json!({})).await {
            Ok(response) => {
                self.log_message(format!("Status: {}", response));
            }
            Err(e) => {
                self.log_message(format!("Status command failed: {}", e));
            }
        }
        Ok(())
    }

    /// Execute /ping command.
    async fn cmd_ping(&mut self) -> Result<(), CliError> {
        match self.ipc.send("runtime.ping", json!({})).await {
            Ok(response) => {
                self.log_message(format!("Pong: {}", response));
            }
            Err(e) => {
                self.log_message(format!("Ping failed: {}", e));
            }
        }
        Ok(())
    }

    /// Execute /shutdown command.
    async fn cmd_shutdown(&mut self) -> Result<(), CliError> {
        match self.ipc.send("runtime.shutdown", json!({})).await {
            Ok(_) => {
                self.log_message("Shutdown initiated. Daemon will disconnect.".to_string());
                self.daemon_status = "Shutting down...".to_string();
                // Give daemon a moment to disconnect, then quit
                tokio::time::sleep(Duration::from_millis(500)).await;
                self.should_quit = true;
            }
            Err(e) => {
                self.log_message(format!("Shutdown command failed: {}", e));
            }
        }
        Ok(())
    }

    /// Execute /models command.
    async fn cmd_list_models(&mut self) -> Result<(), CliError> {
        match self.ipc.send("inference.list_models", json!({})).await {
            Ok(response) => {
                self.log_message(format!("Models: {}", response));
            }
            Err(e) => {
                self.log_message(format!("List models command failed: {}", e));
            }
        }
        Ok(())
    }

    /// Execute /inference command.
    async fn cmd_inference_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("inference.status", json!({})).await {
            Ok(response) => {
                self.log_message(format!("Inference status: {}", response));
            }
            Err(e) => {
                self.log_message(format!("Inference status command failed: {}", e));
            }
        }
        Ok(())
    }

    /// Execute /memory command.
    async fn cmd_memory_stats(&mut self) -> Result<(), CliError> {
        match self.ipc.send("memory.stats", json!({})).await {
            Ok(response) => {
                self.log_message(format!("Memory stats: {}", response));
            }
            Err(e) => {
                self.log_message(format!("Memory stats command failed: {}", e));
            }
        }
        Ok(())
    }

    /// Execute /speech command.
    async fn cmd_speech_status(&mut self) -> Result<(), CliError> {
        match self.ipc.send("speech.status", json!({})).await {
            Ok(response) => {
                self.log_message(format!("Speech status: {}", response));
            }
            Err(e) => {
                self.log_message(format!("Speech status command failed: {}", e));
            }
        }
        Ok(())
    }

    /// Execute /config command.
    async fn cmd_config(&mut self, key: Option<&str>, value: Option<&str>) -> Result<(), CliError> {
        match (key, value) {
            (None, None) => {
                // Open config editor
                self.open_config_editor().await?;
            }
            (Some(key), None) => {
                // Get config value
                match self.ipc.send("config.get", json!({"key": key})).await {
                    Ok(response) => {
                        self.log_message(format!("Config {}: {}", key, response));
                    }
                    Err(e) => {
                        self.log_message(format!("Config get failed: {}", e));
                    }
                }
            }
            (Some(key), Some(value)) => {
                // Set config value
                match self
                    .ipc
                    .send("config.set", json!({"key": key, "value": value}))
                    .await
                {
                    Ok(_) => {
                        self.log_message(format!("Config {} set to {}", key, value));
                    }
                    Err(e) => {
                        self.log_message(format!("Config set failed: {}", e));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Execute /events command.
    async fn cmd_events_subscribe(&mut self) -> Result<(), CliError> {
        match self
            .ipc
            .send("events.subscribe", json!({"filter": "all"}))
            .await
        {
            Ok(response) => {
                self.log_message(format!("Event subscription: {}", response));
                self.log_message("Note: Push event support is partial in Phase 4".to_string());
            }
            Err(e) => {
                self.log_message(format!("Events subscribe failed: {}", e));
            }
        }
        Ok(())
    }

    /// Open interactive config editor.
    async fn open_config_editor(&mut self) -> Result<(), CliError> {
        // Cleanup terminal for config editor
        self.cleanup_terminal()?;

        // Run config editor
        let mut editor = ConfigEditor::new(&mut self.ipc);
        editor.run().await?;

        // Re-setup terminal
        enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let backend = CrosstermBackend::new(stdout);
        self.terminal =
            Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        self.log_message("Config editor closed".to_string());
        Ok(())
    }

    /// Add a message to the log.
    fn log_message(&mut self, message: String) {
        if let Ok(mut log) = self.message_log.lock() {
            log.push(message);
            // Keep log size reasonable
            if log.len() > 500 {
                log.drain(0..100);
            }
        }
    }

    /// Render the TUI (static function to avoid borrow issues).
    fn render_tui(
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        message_log: &[String],
        input_buffer: &str,
        daemon_status: &str,
    ) -> Result<(), io::Error> {
        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Header
                    Constraint::Min(0),    // Message log
                    Constraint::Length(3), // Input
                ])
                .split(frame.area());

            Self::render_header(frame, chunks[0], daemon_status);
            Self::render_message_log(frame, chunks[1], message_log);
            Self::render_input(frame, chunks[2], input_buffer);
        })?;
        Ok(())
    }

    /// Render header with status.
    fn render_header(frame: &mut Frame, area: Rect, daemon_status: &str) {
        let header_text = Line::from(vec![
            Span::styled(
                "Sena CLI",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" | "),
            Span::styled("Daemon: ", Style::default().fg(Color::Gray)),
            Span::styled(
                daemon_status,
                if daemon_status == "Connected" {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
        ]);

        let header = Paragraph::new(header_text)
            .block(Block::default().borders(Borders::ALL).title("Status"));

        frame.render_widget(header, area);
    }

    /// Render message log.
    fn render_message_log(frame: &mut Frame, area: Rect, message_log: &[String]) {
        let messages: Vec<ListItem> = message_log
            .iter()
            .rev()
            .take(area.height.saturating_sub(2) as usize)
            .rev()
            .map(|msg| ListItem::new(msg.as_str()))
            .collect();

        let messages_list =
            List::new(messages).block(Block::default().borders(Borders::ALL).title("Messages"));

        frame.render_widget(messages_list, area);
    }

    /// Render input buffer.
    fn render_input(frame: &mut Frame, area: Rect, input_buffer: &str) {
        let input_text = Paragraph::new(input_buffer)
            .block(Block::default().borders(Borders::ALL).title("Input"))
            .style(Style::default().fg(Color::White));

        frame.render_widget(input_text, area);
    }

    /// Cleanup terminal state.
    fn cleanup_terminal(&mut self) -> Result<(), CliError> {
        disable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )
        .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        self.terminal
            .show_cursor()
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        Ok(())
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // Best-effort cleanup on drop
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
    }
}
