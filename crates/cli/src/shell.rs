//! Ratatui-powered TUI shell for Sena — full-screen interactive interface.
//!
//! Features:
//! - Full-screen TUI with header, conversation log, and input area
//! - Scrollable conversation history
//! - Session statistics display
//! - Ctrl+C double-press to exit
//! - Verbose mode for actor event logging
//! - All transparency queries and model selection

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bus::events::inference::{InferenceEvent, Priority};
use bus::events::transparency::{
    InferenceExplanationResponse, MemoryResponse, ObservationResponse, TransparencyEvent,
    TransparencyQuery,
};
use bus::{Event, TransparencyEvent as BusTransparencyEvent};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};

use crate::tui_state::{ActorStatus, EditorState, Message, MessageRole, SessionStats};
use crate::{display, model_selector};
use runtime::boot::Runtime;

// ── Slash-command autocomplete ────────────────────────────────────────────────

struct SlashCommand {
    command: &'static str,
    description: &'static str,
}

const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        command: "/observation",
        description: "What are you observing right now?",
    },
    SlashCommand {
        command: "/memory",
        description: "What do you remember about me?",
    },
    SlashCommand {
        command: "/explanation",
        description: "Why did you say that?",
    },
    SlashCommand {
        command: "/models",
        description: "Select which model to use",
    },
    SlashCommand {
        command: "/actors",
        description: "Show actor health status",
    },
    SlashCommand {
        command: "/verbose",
        description: "Toggle verbose logging",
    },
    SlashCommand {
        command: "/help",
        description: "Show all commands",
    },
    SlashCommand {
        command: "/quit",
        description: "Exit Sena",
    },
];

struct SlashDropdown {
    filtered: Vec<usize>,
    selected: usize,
}

impl SlashDropdown {
    fn from_prefix(prefix: &str) -> Self {
        let lower = prefix.to_lowercase();
        let filtered: Vec<usize> = SLASH_COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, c)| c.command.starts_with(lower.as_str()))
            .map(|(i, _)| i)
            .collect();
        Self {
            filtered,
            selected: 0,
        }
    }

    fn update(&mut self, prefix: &str) {
        let lower = prefix.to_lowercase();
        self.filtered = SLASH_COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, c)| c.command.starts_with(lower.as_str()))
            .map(|(i, _)| i)
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
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

    fn next(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.filtered.len();
    }

    fn selected_command(&self) -> Option<&'static str> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| SLASH_COMMANDS.get(i))
            .map(|c| c.command)
    }

    fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }
}

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
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
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
    /// Model selector popup (visible when not None).
    model_popup: Option<model_selector::ModelSelectorPopup>,
    /// Actor health status tracking.
    actor_health: HashMap<&'static str, ActorStatus>,
    /// Actors health popup visibility flag.
    actors_popup_visible: bool,
    /// Slash-command autocomplete dropdown (visible when input starts with '/').
    slash_dropdown: Option<SlashDropdown>,
    /// True while waiting for a transparency query response on the bus.
    transparency_loading: bool,
    /// Runtime reference for config access.
    runtime: Arc<Runtime>,
}

impl Shell {
    /// Create a new Shell instance.
    fn new(runtime: Arc<Runtime>) -> Self {
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

        // Pre-populate actor health with all known actors as Starting
        let mut actor_health = HashMap::new();
        actor_health.insert("Platform", ActorStatus::Starting);
        actor_health.insert("Inference", ActorStatus::Starting);
        actor_health.insert("CTP", ActorStatus::Starting);
        actor_health.insert("Memory", ActorStatus::Starting);
        actor_health.insert("Soul", ActorStatus::Starting);

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
            model_popup: None,
            actor_health,
            actors_popup_visible: false,
            slash_dropdown: None,
            transparency_loading: false,
            runtime,
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

        // Model selector popup (rendered on top if visible)
        if let Some(popup) = &self.model_popup {
            model_selector::render_popup(popup, frame);
        }

        // Slash dropdown (rendered above input area, below other popups)
        if self.model_popup.is_none() && !self.actors_popup_visible {
            self.render_slash_dropdown(frame, chunks[2]);
        }

        // Actors health popup (rendered on top if visible)
        if self.actors_popup_visible {
            self.render_actors_popup(frame);
        }
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
        } else if self.transparency_loading {
            Line::from(Span::styled(
                "⏳ Loading...",
                Style::default().fg(Color::Yellow),
            ))
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
        let model_part = self
            .current_model
            .as_deref()
            .map(|m| format!(" • {}", m))
            .unwrap_or_default();
        let verbose_part = if self.verbose { " [verbose]" } else { "" };
        let text = format!("Ready{}{}", model_part, verbose_part);
        Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
    }

    /// Render the actors health popup.
    fn render_actors_popup(&self, frame: &mut ratatui::Frame) {
        use ratatui::widgets::{Cell, Clear, Row, Table};

        // Create a centered popup (60% width, 50% height)
        let area = centered_rect(60, 50, frame.area());

        // Clear the area
        frame.render_widget(Clear, area);

        // Build table rows — fixed order: Platform, Inference, CTP, Memory, Soul
        let actor_names = ["Platform", "Inference", "CTP", "Memory", "Soul"];
        let mut rows = Vec::new();

        for name in &actor_names {
            let status = self
                .actor_health
                .get(name)
                .unwrap_or(&ActorStatus::Starting);
            let (symbol, status_text, status_color) = match status {
                ActorStatus::Ready => ("✓", "Ready", Color::Green),
                ActorStatus::Starting => ("◦", "Starting", Color::Yellow),
                ActorStatus::Failed(reason) => ("✗", reason.as_str(), Color::Red),
            };

            rows.push(Row::new(vec![
                Cell::from(name.to_string()),
                Cell::from(format!("{} {}", symbol, status_text))
                    .style(Style::default().fg(status_color)),
            ]));
        }

        let widths = [Constraint::Percentage(30), Constraint::Percentage(70)];
        let table = Table::new(rows, widths)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title("  Actor Health  "),
            )
            .header(
                Row::new(vec!["Actor", "Status"])
                    .style(Style::default().add_modifier(Modifier::BOLD))
                    .bottom_margin(1),
            )
            .column_spacing(2);

        frame.render_widget(table, area);

        // Render footer hint at bottom of popup
        let footer_area = ratatui::layout::Rect {
            x: area.x + 2,
            y: area.y + area.height.saturating_sub(2),
            width: area.width.saturating_sub(4),
            height: 1,
        };

        let footer = Paragraph::new(Line::from(Span::styled(
            "[any key to dismiss]",
            Style::default().fg(Color::DarkGray),
        )));

        frame.render_widget(footer, footer_area);
    }

    /// Render the slash command autocomplete dropdown just above the input area.
    fn render_slash_dropdown(&self, frame: &mut ratatui::Frame, input_area: ratatui::layout::Rect) {
        use ratatui::widgets::Clear;

        let Some(ref dd) = self.slash_dropdown else {
            return;
        };
        if dd.is_empty() {
            return;
        }

        let count = dd.filtered.len() as u16;
        let popup_height = count.min(8) + 2; // +2 for border
        let popup_width = 62u16.min(frame.area().width.saturating_sub(4));

        // Position immediately above the input area
        let y = input_area.y.saturating_sub(popup_height);
        let popup_area = ratatui::layout::Rect {
            x: input_area.x + 2,
            y,
            width: popup_width,
            height: popup_height,
        };

        frame.render_widget(Clear, popup_area);

        let items: Vec<ListItem> = dd
            .filtered
            .iter()
            .map(|&i| {
                let cmd = &SLASH_COMMANDS[i];
                ListItem::new(Line::from(vec![
                    Span::styled(
                        cmd.command,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(cmd.description, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title("  Tab \u{2508} complete  \u{2191}\u{2193} \u{2508} navigate  Esc \u{2508} close  "),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );

        let mut state = ListState::default();
        state.select(Some(dd.selected));
        frame.render_stateful_widget(list, popup_area, &mut state);
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
            Event::System(bus::events::SystemEvent::ActorReady { actor_name }) => {
                if let Some(status) = self.actor_health.get_mut(actor_name) {
                    *status = ActorStatus::Ready;
                }
                if self.verbose {
                    self.add_message(
                        MessageRole::System,
                        format!("[verbose] Actor ready: {}", actor_name),
                    );
                }
            }
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
            // ── Transparency query responses (async, non-blocking) ─────────────
            Event::Transparency(TransparencyEvent::ObservationResponded(resp)) => {
                self.transparency_loading = false;
                self.add_message(
                    MessageRole::System,
                    "\u{2501}\u{2501}  Current Observation".to_string(),
                );
                self.add_message(MessageRole::Sena, format_observation_tui(resp));
            }
            Event::Transparency(TransparencyEvent::MemoryResponded(resp)) => {
                self.transparency_loading = false;
                self.add_message(MessageRole::System, "\u{2501}\u{2501}  Memory".to_string());
                self.add_message(MessageRole::Sena, format_memory_tui(resp));
            }
            Event::Transparency(TransparencyEvent::InferenceExplanationResponded(resp)) => {
                self.transparency_loading = false;
                self.add_message(
                    MessageRole::System,
                    "\u{2501}\u{2501}  Last Inference".to_string(),
                );
                self.add_message(MessageRole::Sena, format_explanation_tui(resp));
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
                self.fire_transparency_query(
                    TransparencyQuery::CurrentObservation,
                    "Querying current observation...",
                )
                .await;
                DispatchResult::Continue
            }
            "/memory" | "/mem" => {
                self.fire_transparency_query(TransparencyQuery::UserMemory, "Querying memory...")
                    .await;
                DispatchResult::Continue
            }
            "/explanation" | "/why" => {
                self.fire_transparency_query(
                    TransparencyQuery::InferenceExplanation,
                    "Querying last inference...",
                )
                .await;
                DispatchResult::Continue
            }
            "/models" => {
                match model_selector::discover_popup().await {
                    Ok(popup) => {
                        self.model_popup = Some(popup);
                    }
                    Err(e) => {
                        self.add_message(
                            MessageRole::Warning,
                            format!("Model discovery failed: {}", e),
                        );
                    }
                }
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
                self.actors_popup_visible = true;
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

    /// Fire a transparency query on the bus and return immediately.
    /// The response arrives asynchronously via handle_bus_event.
    async fn fire_transparency_query(&mut self, query: TransparencyQuery, loading_msg: &str) {
        self.add_message(MessageRole::System, loading_msg.to_string());
        self.transparency_loading = true;
        if let Err(e) = self
            .bus
            .broadcast(Event::Transparency(BusTransparencyEvent::QueryRequested(
                query,
            )))
            .await
        {
            self.transparency_loading = false;
            self.add_message(MessageRole::Warning, format!("Failed to send query: {}", e));
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
            "/actors                Show actor health status".to_string(),
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
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let _guard = TerminalGuard; // Ensures cleanup on drop

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // ── Initialize shell state ────────────────────────────────────────────────
    let runtime = Arc::new(runtime);
    let mut shell = Shell::new(runtime.clone());

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
                    match event::read()? {
                        // ── Mouse scroll ─────────────────────────────────────
                        event::Event::Mouse(mouse) => {
                            match mouse.kind {
                                event::MouseEventKind::ScrollUp => {
                                    shell.scroll_offset = shell.scroll_offset.saturating_add(3);
                                }
                                event::MouseEventKind::ScrollDown => {
                                    shell.scroll_offset = shell.scroll_offset.saturating_sub(3);
                                    // Snap to bottom when at bottom
                                    if shell.scroll_offset == 0 {
                                        shell.scroll_offset = 0;
                                    }
                                }
                                _ => {}
                            }
                        }

                        event::Event::Key(key) => {
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

                            // ── Model Popup Key Handlers ──────────────────────────────
                            // When popup is visible, intercept navigation keys
                            (KeyCode::Up, _) if shell.model_popup.is_some() => {
                                if let Some(popup) = &mut shell.model_popup {
                                    popup.prev();
                                }
                            }
                            (KeyCode::Down, _) if shell.model_popup.is_some() => {
                                if let Some(popup) = &mut shell.model_popup {
                                    popup.next();
                                }
                            }
                            (KeyCode::Enter, _) if shell.model_popup.is_some() => {
                                // Apply selection
                                if let Some(popup) = shell.model_popup.take() {
                                    if let Some(selected) = popup.selected() {
                                        let model_name = selected.name.clone();
                                        // Update config
                                        let mut config = shell.runtime.config.clone();
                                        config.preferred_model = Some(model_name.clone());
                                        match runtime::save_config(&config).await {
                                            Ok(_) => {
                                                shell.add_message(
                                                    MessageRole::System,
                                                    format!("Selected model: {}", model_name),
                                                );
                                                shell.add_message(
                                                    MessageRole::System,
                                                    "Model change will take effect after restart.".to_string(),
                                                );
                                            }
                                            Err(e) => {
                                                shell.add_message(
                                                    MessageRole::Warning,
                                                    format!("Failed to save config: {}", e),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            (KeyCode::Esc, _) if shell.model_popup.is_some() => {
                                // Cancel popup
                                shell.model_popup = None;
                                shell.add_message(
                                    MessageRole::System,
                                    "Model selection cancelled.".to_string(),
                                );
                            }

                            // ── Actors Popup Key Handlers ─────────────────────────────
                            // Any key dismisses the actors health popup
                            _ if shell.actors_popup_visible && shell.model_popup.is_none() => {
                                shell.actors_popup_visible = false;
                            }

                            // ── Slash Dropdown Key Handlers ───────────────────────────
                            (KeyCode::Up, _) if shell.slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                if let Some(dd) = &mut shell.slash_dropdown {
                                    dd.prev();
                                }
                            }
                            (KeyCode::Down, _) if shell.slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                if let Some(dd) = &mut shell.slash_dropdown {
                                    dd.next();
                                }
                            }
                            (KeyCode::Tab, _) if shell.slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                // Complete the command into the input
                                if let Some(cmd) = shell.slash_dropdown.as_ref().and_then(|d| d.selected_command()) {
                                    shell.editor.input = cmd.to_string();
                                    shell.slash_dropdown = None;
                                }
                            }
                            (KeyCode::Enter, _) if shell.slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                // Complete and immediately submit
                                if let Some(cmd) = shell.slash_dropdown.as_ref().and_then(|d| d.selected_command()) {
                                    let line = cmd.to_string();
                                    shell.editor.input.clear();
                                    shell.slash_dropdown = None;
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
                            (KeyCode::Esc, _) if shell.slash_dropdown.is_some() => {
                                shell.slash_dropdown = None;
                            }

                            // ── Normal Key Handlers (when popup is NOT visible) ──────
                            // Enter — submit line
                            (KeyCode::Enter, _) => {
                                let line = shell.editor.input.trim().to_string();
                                shell.editor.input.clear();
                                shell.slash_dropdown = None;
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
                            (KeyCode::Backspace, _) if shell.model_popup.is_none() => {
                                shell.editor.input.pop();
                                // Update slash dropdown after backspace
                                if shell.editor.input.starts_with('/') {
                                    if let Some(dd) = &mut shell.slash_dropdown {
                                        dd.update(&shell.editor.input);
                                        if dd.is_empty() {
                                            shell.slash_dropdown = None;
                                        }
                                    } else {
                                        let dd = SlashDropdown::from_prefix(&shell.editor.input);
                                        if !dd.is_empty() {
                                            shell.slash_dropdown = Some(dd);
                                        }
                                    }
                                } else {
                                    shell.slash_dropdown = None;
                                }
                            }

                            // Arrow Up — history prev (only when dropdown not active)
                            (KeyCode::Up, _) if shell.model_popup.is_none() && shell.slash_dropdown.is_none() => {
                                shell.editor.history_prev();
                            }

                            // Arrow Down — history next (only when dropdown not active)
                            (KeyCode::Down, _) if shell.model_popup.is_none() && shell.slash_dropdown.is_none() => {
                                shell.editor.history_next();
                            }

                            // Page Up — scroll up
                            (KeyCode::PageUp, _) if shell.model_popup.is_none() => {
                                shell.scroll_offset = shell.scroll_offset.saturating_add(10);
                            }

                            // Page Down — scroll down
                            (KeyCode::PageDown, _) if shell.model_popup.is_none() => {
                                shell.scroll_offset = shell.scroll_offset.saturating_sub(10);
                            }

                            // Escape — clear input and close dropdown
                            (KeyCode::Esc, _) if shell.model_popup.is_none() => {
                                shell.editor.input.clear();
                                shell.slash_dropdown = None;
                            }

                            // Regular character
                            (KeyCode::Char(c), mods) if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) && shell.model_popup.is_none() => {
                                shell.editor.input.push(c);
                                // Reset Ctrl+C first press if user is typing
                                shell.ctrl_c_first_press = None;
                                // Update slash dropdown on every character typed
                                if shell.editor.input.starts_with('/') {
                                    if let Some(dd) = &mut shell.slash_dropdown {
                                        dd.update(&shell.editor.input);
                                        if dd.is_empty() {
                                            shell.slash_dropdown = None;
                                        }
                                    } else {
                                        let dd = SlashDropdown::from_prefix(&shell.editor.input);
                                        if !dd.is_empty() {
                                            shell.slash_dropdown = Some(dd);
                                        }
                                    }
                                } else {
                                    shell.slash_dropdown = None;
                                }
                            }

                            _ => {}
                        }
                        } // end event::Event::Key

                        _ => {} // resize, focus, paste — ignore
                    }
                }
            }
        }
    }

    // ── Graceful shutdown ─────────────────────────────────────────────────────
    drop(_guard); // Restore terminal
    drop(terminal); // Drop terminal before printing to stdout

    // Extract stats before dropping shell
    let stats = shell.stats.clone();

    // Drop shell to release Arc<Runtime> reference
    let timeout_secs = runtime.config.shutdown_timeout_secs;
    drop(shell);

    // Print session summary only for Quit (not Restart)
    if exit_reason == ShellExitReason::Quit {
        display::print_session_summary(&stats);
    }

    println!();
    display::info("Shutting down actors...");

    // Extract runtime from Arc for shutdown (should succeed after dropping shell)
    let runtime_owned = Arc::try_unwrap(runtime)
        .map_err(|_| anyhow::anyhow!("runtime has remaining references at shutdown"))?;
    let timeout = Duration::from_secs(timeout_secs);
    runtime::shutdown(runtime_owned, timeout).await?;
    display::success("Sena stopped cleanly.");

    Ok(exit_reason)
}

/// Helper to create a centered rect using percentage of the available rect.
fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Format ObservationResponse for TUI display (no ANSI codes).
fn format_observation_tui(resp: &ObservationResponse) -> String {
    let snapshot = &resp.snapshot;
    let app = &snapshot.active_app.app_name;
    let task = match &snapshot.inferred_task {
        Some(hint) => format!("{} ({:.0}%)", hint.category, hint.confidence * 100.0),
        None => "(no task inferred)".to_string(),
    };
    let clipboard = if snapshot.clipboard_digest.is_some() {
        "ready"
    } else {
        "empty"
    };
    let rate = snapshot.keystroke_cadence.events_per_minute;
    let secs = snapshot.session_duration.as_secs();
    let session = if secs >= 60 {
        format!("{} min {} sec", secs / 60, secs % 60)
    } else {
        format!("{secs} sec")
    };
    format!(
        "Window     {app}\nTask       {task}\nClipboard  {clipboard}\nKeyboard   {rate:.1} events/min\nSession    {session}"
    )
}

/// Format MemoryResponse for TUI display (no ANSI codes).
fn format_memory_tui(resp: &MemoryResponse) -> String {
    let summary = &resp.soul_summary;
    let patterns = if summary.work_patterns.is_empty() {
        "(none detected)".to_string()
    } else {
        summary.work_patterns.join(", ")
    };
    let preferences = if summary.tool_preferences.is_empty() {
        "(none detected)".to_string()
    } else {
        summary.tool_preferences.join(", ")
    };
    let interests = if summary.interest_clusters.is_empty() {
        "(none detected)".to_string()
    } else {
        summary.interest_clusters.join(", ")
    };

    let mut out = format!(
        "Soul Summary\nWork patterns  {patterns}\nTools          {preferences}\nInterests      {interests}"
    );
    out.push_str("\n\nRecent Memories");
    if resp.memory_chunks.is_empty() {
        out.push_str("\n  (no memories retrieved)");
    } else {
        for (i, chunk) in resp.memory_chunks.iter().enumerate() {
            let preview = if chunk.text.len() > 120 {
                format!("{}...", &chunk.text[..120])
            } else {
                chunk.text.clone()
            };
            out.push_str(&format!(
                "\n  [{}] {preview}\n       score: {:.2}",
                i + 1,
                chunk.score
            ));
        }
    }
    out
}

/// Format InferenceExplanationResponse for TUI display (no ANSI codes).
fn format_explanation_tui(resp: &InferenceExplanationResponse) -> String {
    let request = if resp.request_context.len() > 200 {
        format!("{}...", &resp.request_context[..200])
    } else {
        resp.request_context.clone()
    };
    let response = if resp.response_text.len() > 299 {
        format!("{}...", &resp.response_text[..299])
    } else {
        resp.response_text.clone()
    };
    let mut out = format!(
        "Rounds: {}\nRequest   {request}\nResponse  {response}",
        resp.rounds_completed
    );
    if resp.working_memory_context.is_empty() {
        out.push_str("\nMemory    (none used)");
    } else {
        out.push_str(&format!(
            "\nMemory    {} chunks used",
            resp.working_memory_context.len()
        ));
        for (i, chunk) in resp.working_memory_context.iter().enumerate() {
            let preview = if chunk.text.len() > 80 {
                format!("{}...", &chunk.text[..80])
            } else {
                chunk.text.clone()
            };
            out.push_str(&format!("\n          [{}] {preview}", i + 1));
        }
    }
    out
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
