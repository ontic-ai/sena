//! Ratatui-powered TUI shell for Sena — full-screen interactive interface.
//!
//! Features:
//! - Full-screen TUI with header, conversation log, and input area
//! - Scrollable conversation history
//! - Session statistics display
//! - Ctrl+C double-press to exit
//! - Verbose mode for actor event logging
//! - All transparency queries and model selection

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bus::events::inference::{InferenceEvent, Priority};
use bus::events::transparency::TransparencyQuery;
use bus::Event;
use crossterm::{
    event::{self, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};

use crate::tui_state::{EditorState, Message, MessageRole, SessionStats};
use crate::{display, query};
use runtime::boot::Runtime;

/// Reason the shell exited — drives the restart loop in main.rs.
#[derive(Debug, PartialEq)]
pub enum ShellExitReason {
    Quit,
    Restart,
}

/// RAII guard — restores the terminal unconditionally when dropped.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Main TUI shell state.
struct Shell {
    /// Event bus for actor communication.
    bus: Arc<bus::EventBus>,
    /// Input line editor with history.
    editor: EditorState,
    /// Conversation log messages.
    messages: Vec<Message>,
    /// Scroll offset from bottom (0 = at bottom, autoscroll).
    scroll_offset: usize,
    /// Session statistics.
    stats: SessionStats,
    /// First Ctrl+C press timestamp for double-press detection.
    ctrl_c_first_press: Option<Instant>,
    /// Are we waiting for an inference response?
    waiting_for_inference: bool,
    /// Request ID of the pending inference (if any).
    pending_inference_id: Option<u64>,
    /// Verbose mode: show all actor events.
    verbose: bool,
    /// Currently loaded model name.
    current_model: Option<String>,
}

impl Shell {
    /// Create a new Shell instance.
    fn new(runtime: &Runtime) -> Self {
        let messages = vec![
            Message::new(
                MessageRole::System,
                "Welcome to Sena — local-first ambient AI".to_string(),
            ),
            Message::new(
                MessageRole::System,
                "Type /help for commands, or chat freely.".to_string(),
            ),
        ];

        Self {
            bus: runtime.bus.clone(),
            editor: EditorState::new(),
            messages,
            scroll_offset: 0,
            stats: SessionStats::new(),
            ctrl_c_first_press: None,
            waiting_for_inference: false,
            pending_inference_id: None,
            verbose: false,
            current_model: runtime.config.preferred_model.clone(),
        }
    }

    /// Render the TUI.
    fn render(&self, frame: &mut ratatui::Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(1),    // Conversation log
                Constraint::Length(3), // Input + status
            ])
            .split(frame.area());

        // Header
        self.render_header(frame, chunks[0]);

        // Conversation log
        self.render_conversation(frame, chunks[1]);

        // Input area
        self.render_input(frame, chunks[2]);
    }

    /// Render the header section.
    fn render_header(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let elapsed = self.stats.elapsed_formatted();
        let model = self.current_model.as_deref().unwrap_or("(auto-selecting)");

        let header_text = vec![
            Line::from(vec![
                Span::styled(
                    "SENA",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" — local-first ambient AI"),
            ]),
            Line::from(vec![
                Span::raw("Session: "),
                Span::styled(elapsed, Style::default().fg(Color::Green)),
                Span::raw("  •  Messages: "),
                Span::styled(
                    self.stats.messages_sent.to_string(),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw("  •  Tokens: "),
                Span::styled(
                    self.stats.tokens_received.to_string(),
                    Style::default().fg(Color::Magenta),
                ),
                Span::raw("  •  Model: "),
                Span::styled(model, Style::default().fg(Color::Cyan)),
            ]),
        ];

        let header = Paragraph::new(header_text).block(Block::default().borders(Borders::BOTTOM));

        frame.render_widget(header, area);
    }

    /// Render the conversation log.
    fn render_conversation(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let mut lines = Vec::new();

        for msg in &self.messages {
            match msg.role {
                MessageRole::User => {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "> ",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(&msg.text),
                    ]));
                    lines.push(Line::from("")); // Spacing
                }
                MessageRole::Sena => {
                    // Multi-line response wrapping
                    for line in msg.text.lines() {
                        lines.push(Line::from(Span::styled(
                            line,
                            Style::default().fg(Color::Green),
                        )));
                    }
                    lines.push(Line::from("")); // Spacing
                }
                MessageRole::System => {
                    lines.push(Line::from(Span::styled(
                        &msg.text,
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
                MessageRole::Warning => {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "⚠ ",
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(&msg.text, Style::default().fg(Color::Yellow)),
                    ]));
                }
            }
        }

        // Calculate scroll position
        let total_lines = lines.len();
        let visible_lines = area.height.saturating_sub(2) as usize; // Account for block borders
        let scroll = if self.scroll_offset == 0 {
            // Auto-scroll to bottom
            total_lines.saturating_sub(visible_lines)
        } else {
            // User has scrolled up
            total_lines.saturating_sub(visible_lines + self.scroll_offset)
        };

        let text = Text::from(lines);
        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));

        frame.render_widget(paragraph, area);
    }

    /// Render the input area and status line.
    fn render_input(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let input_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Separator
                Constraint::Length(1), // Input line
                Constraint::Length(1), // Status line
            ])
            .split(area);

        // Top separator
        let separator = Paragraph::new("").block(Block::default().borders(Borders::TOP));
        frame.render_widget(separator, input_chunks[0]);

        // Input line
        let input_text = Line::from(vec![
            Span::styled(
                "sena",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" › ", Style::default().fg(Color::DarkGray)),
            Span::raw(&self.editor.input),
            Span::styled("_", Style::default().fg(Color::DarkGray)),
        ]);
        let input_paragraph = Paragraph::new(input_text);
        frame.render_widget(input_paragraph, input_chunks[1]);

        // Status line
        let status_text = if let Some(first_press) = self.ctrl_c_first_press {
            if first_press.elapsed() < Duration::from_secs(3) {
                Line::from(Span::styled(
                    "Press Ctrl+C again to exit",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                self.status_line_normal()
            }
        } else if self.waiting_for_inference {
            Line::from(Span::styled(
                "⏳ Thinking...",
                Style::default().fg(Color::Yellow),
            ))
        } else {
            self.status_line_normal()
        };

        let status_paragraph = Paragraph::new(status_text);
        frame.render_widget(status_paragraph, input_chunks[2]);
    }

    /// Generate the normal status line.
    fn status_line_normal(&self) -> Line<'static> {
        if self.verbose {
            Line::from(Span::styled(
                "Ready [verbose on]",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(Span::styled("Ready", Style::default().fg(Color::DarkGray)))
        }
    }

    /// Add a message to the conversation log.
    fn add_message(&mut self, role: MessageRole, text: String) {
        self.messages.push(Message::new(role, text));
        // Auto-scroll to bottom when new message arrives (unless user scrolled up)
        if self.scroll_offset == 0 {
            // Already at bottom, stay there
        }
    }

    /// Handle bus events and update internal state.
    fn handle_bus_event(&mut self, event: &Event) {
        match event {
            Event::Inference(InferenceEvent::InferenceCompleted {
                text,
                request_id,
                token_count,
                ..
            }) if self.pending_inference_id == Some(*request_id) => {
                self.pending_inference_id = None;
                self.waiting_for_inference = false;
                if text.trim().is_empty() {
                    self.add_message(
                        MessageRole::Warning,
                        "Model returned empty response".to_string(),
                    );
                } else {
                    self.add_message(MessageRole::Sena, text.clone());
                    self.stats.tokens_received += token_count;
                }
            }
            Event::Inference(InferenceEvent::InferenceFailed { request_id, reason })
                if self.pending_inference_id == Some(*request_id) =>
            {
                self.pending_inference_id = None;
                self.waiting_for_inference = false;
                self.add_message(
                    MessageRole::Warning,
                    format!("Inference failed: {}", reason),
                );
            }
            Event::Inference(InferenceEvent::ModelLoaded { name, backend }) => {
                if self.verbose || self.waiting_for_inference {
                    self.add_message(
                        MessageRole::System,
                        format!("Model loaded: {} ({})", name, backend),
                    );
                }
                self.current_model = Some(name.clone());
            }
            Event::Inference(InferenceEvent::BackendMismatchWarning { detected, compiled }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!(
                        "GPU not active: detected {} but compiled {}",
                        detected, compiled
                    ),
                );
            }
            Event::System(bus::events::SystemEvent::BootComplete) => {
                if self.verbose {
                    self.add_message(MessageRole::System, "Boot complete".to_string());
                }
            }
            _ if self.verbose => {
                if let Some(msg) = verbose_format(event) {
                    self.add_message(MessageRole::System, msg);
                }
            }
            _ => {}
        }
    }

    /// Dispatch user input (command or chat).
    async fn dispatch_line(&mut self, line: String) -> DispatchResult {
        let lower = line.to_lowercase();
        #[allow(clippy::manual_unwrap_or_default, clippy::manual_unwrap_or)]
        let cmd = if let Some(v) = lower.split_whitespace().next() {
            v
        } else {
            ""
        };

        match cmd {
            "/observation" | "/obs" => {
                self.run_query(TransparencyQuery::CurrentObservation).await;
                DispatchResult::Continue
            }
            "/memory" | "/mem" => {
                self.run_query(TransparencyQuery::UserMemory).await;
                DispatchResult::Continue
            }
            "/explanation" | "/why" => {
                self.run_query(TransparencyQuery::InferenceExplanation)
                    .await;
                DispatchResult::Continue
            }
            "/models" => {
                self.add_message(
                    MessageRole::System,
                    "Model selection via TUI not yet implemented — use /load <n> for now"
                        .to_string(),
                );
                DispatchResult::Continue
            }
            "/verbose" => {
                self.verbose = !self.verbose;
                let state = if self.verbose { "ON" } else { "OFF" };
                self.add_message(MessageRole::System, format!("Verbose logging: {}", state));
                DispatchResult::Continue
            }
            "/help" | "/h" => {
                self.show_help();
                DispatchResult::Continue
            }
            "/actors" => {
                self.add_message(
                    MessageRole::System,
                    "Actor health: use the /actors command (coming soon)".to_string(),
                );
                DispatchResult::Continue
            }
            "/quit" | "/exit" | "/q" => DispatchResult::Quit,
            _ if line.starts_with('/') => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Unknown command '{}'. Type /help for commands.", line),
                );
                DispatchResult::Continue
            }
            _ => {
                // Free text → inference chat
                self.send_chat(line).await;
                DispatchResult::Continue
            }
        }
    }

    /// Run a transparency query.
    async fn run_query(&mut self, query: TransparencyQuery) {
        let label = match &query {
            TransparencyQuery::CurrentObservation => "Current Observation",
            TransparencyQuery::UserMemory => "Memory",
            TransparencyQuery::InferenceExplanation => "Last Inference",
        };

        self.add_message(MessageRole::System, format!("Querying {}...", label));

        match query::query_on_bus(query, &self.bus).await {
            Ok(output) => {
                self.add_message(MessageRole::System, format!("━━  {}", label));
                self.add_message(MessageRole::Sena, output);
            }
            Err(e) => {
                self.add_message(MessageRole::Warning, format!("Query failed: {}", e));
            }
        }
    }

    /// Send a chat message to the inference actor.
    async fn send_chat(&mut self, prompt: String) {
        // Add user message to log
        self.add_message(MessageRole::User, prompt.clone());

        // Generate request ID
        let request_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);

        self.add_message(MessageRole::System, "Thinking...".to_string());
        self.waiting_for_inference = true;
        self.pending_inference_id = Some(request_id);
        self.stats.messages_sent += 1;

        if let Err(e) = self
            .bus
            .send_directed(
                "inference",
                Event::Inference(InferenceEvent::InferenceRequested {
                    prompt,
                    priority: Priority::High,
                    request_id,
                }),
            )
            .await
        {
            self.waiting_for_inference = false;
            self.pending_inference_id = None;
            self.add_message(
                MessageRole::Warning,
                format!("Could not reach inference actor: {}", e),
            );
        }
    }

    /// Show help text.
    fn show_help(&mut self) {
        self.add_message(MessageRole::System, "━━  Commands".to_string());
        self.add_message(
            MessageRole::System,
            "/observation or /obs   What are you observing right now?".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/memory or /mem        What do you remember about me?".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/explanation or /why   Why did you say that?".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/models                Select which model to use".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/verbose               Toggle verbose actor-event logging".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/actors                Show actor health (coming soon)".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/help                  Show this message".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/quit                  Exit Sena".to_string(),
        );
        self.add_message(MessageRole::System, "".to_string());
        self.add_message(
            MessageRole::System,
            "Type any message to chat with the model.".to_string(),
        );
    }
}

#[allow(dead_code)]
enum DispatchResult {
    Continue,
    Quit,
    Restart,
}

/// Run the interactive shell. Returns the exit reason for the restart loop.
pub async fn run(runtime: Runtime) -> Result<ShellExitReason> {
    // ── Enter raw mode and alternate screen ───────────────────────────────────
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let _guard = TerminalGuard; // Ensures cleanup on drop

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // ── Initialize shell state ────────────────────────────────────────────────
    let mut shell = Shell::new(&runtime);

    // ── Ctrl-C shutdown watch ─────────────────────────────────────────────────
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    // ── Bus subscriber for events ─────────────────────────────────────────────
    let mut bus_rx = runtime.bus.subscribe_broadcast();

    let mut exit_reason = ShellExitReason::Quit;

    // ── Main event loop ───────────────────────────────────────────────────────
    loop {
        // Render the current state
        terminal.draw(|f| shell.render(f))?;

        // Check for Ctrl+C timeout (reset if 3 seconds passed)
        if let Some(first_press) = shell.ctrl_c_first_press {
            if first_press.elapsed() > Duration::from_secs(3) {
                shell.ctrl_c_first_press = None;
            }
        }

        tokio::select! {
            biased;

            // Ctrl-C signal
            _ = shutdown_rx.changed() => {
                if let Some(first_press) = shell.ctrl_c_first_press {
                    if first_press.elapsed() < Duration::from_secs(3) {
                        // Second Ctrl+C within 3 seconds
                        break;
                    }
                } else {
                    shell.ctrl_c_first_press = Some(Instant::now());
                }
            }

            // Bus events
            bcast = bus_rx.recv() => {
                if let Ok(ev) = bcast {
                    shell.handle_bus_event(&ev);
                }
            }

            // Keyboard events (poll in a non-blocking way)
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                // Poll for crossterm events
                if event::poll(Duration::from_millis(0))? {
                    if let event::Event::Key(key) = event::read()? {
                        // Filter out key release events (Windows)
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        match (key.code, key.modifiers) {
                            // Ctrl+C handled via shutdown signal above
                            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                if let Some(first_press) = shell.ctrl_c_first_press {
                                    if first_press.elapsed() < Duration::from_secs(3) {
                                        break;
                                    }
                                } else {
                                    shell.ctrl_c_first_press = Some(Instant::now());
                                }
                            }

            // Ctrl+D
                            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                break;
                            }

                            // Enter — submit line
                            (KeyCode::Enter, _) => {
                                let line = shell.editor.input.trim().to_string();
                                shell.editor.input.clear();
                                if !line.is_empty() {
                                    shell.editor.push_history(&line);
                                    let result = shell.dispatch_line(line).await;
                                    match result {
                                        DispatchResult::Continue => {}
                                        DispatchResult::Quit => {
                                            exit_reason = ShellExitReason::Quit;
                                            break;
                                        }
                                        DispatchResult::Restart => {
                                            exit_reason = ShellExitReason::Restart;
                                            break;
                                        }
                                    }
                                }
                            }

                            // Backspace
                            (KeyCode::Backspace, _) => {
                                shell.editor.input.pop();
                            }

                            // Arrow Up — history prev
                            (KeyCode::Up, _) => {
                                shell.editor.history_prev();
                            }

                            // Arrow Down — history next
                            (KeyCode::Down, _) => {
                                shell.editor.history_next();
                            }

                            // Page Up — scroll up
                            (KeyCode::PageUp, _) => {
                                shell.scroll_offset = shell.scroll_offset.saturating_add(10);
                            }

                            // Page Down — scroll down
                            (KeyCode::PageDown, _) => {
                                shell.scroll_offset = shell.scroll_offset.saturating_sub(10);
                            }

                            // Escape — clear input
                            (KeyCode::Esc, _) => {
                                shell.editor.input.clear();
                            }

                            // Regular character
                            (KeyCode::Char(c), mods) if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) => {
                                shell.editor.input.push(c);
                                // Reset Ctrl+C first press if user is typing
                                shell.ctrl_c_first_press = None;
                            }

                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // ── Graceful shutdown ─────────────────────────────────────────────────────
    drop(_guard); // Restore terminal
    drop(terminal); // Drop terminal before printing to stdout
    println!();
    display::info("Shutting down actors...");
    let timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    runtime::shutdown(runtime, timeout).await?;
    display::success("Sena stopped cleanly.");

    Ok(exit_reason)
}

/// Format bus events for verbose mode.
fn verbose_format(ev: &Event) -> Option<String> {
    match ev {
        Event::CTP(bus::events::CTPEvent::ThoughtEventTriggered(_)) => {
            Some("[verbose] CTP: thought triggered".to_string())
        }
        Event::Soul(bus::events::SoulEvent::EventLogged(e)) => {
            Some(format!("[verbose] Soul: event logged (row {})", e.row_id))
        }
        Event::Platform(bus::events::PlatformEvent::WindowChanged(w)) => {
            Some(format!("[verbose] Window: {}", w.app_name))
        }
        Event::Inference(InferenceEvent::ModelLoaded { name, .. }) => {
            Some(format!("[verbose] Inference: model loaded — {}", name))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_exit_reason_quit_and_restart_are_distinct() {
        assert_ne!(ShellExitReason::Quit, ShellExitReason::Restart);
    }
}
