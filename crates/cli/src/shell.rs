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
use bus::events::speech::SpeechEvent;
use bus::events::transparency::{
    InferenceExplanationResponse, MemoryResponse, ObservationResponse, TransparencyEvent,
    TransparencyQuery,
};
use bus::{DownloadEvent, Event, TransparencyEvent as BusTransparencyEvent};
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
use std::path::PathBuf;

const TRANSPARENCY_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

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
        command: "/copy",
        description: "Copy the last response to clipboard",
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
        command: "/voice",
        description: "Toggle voice input in this CLI session",
    },
    SlashCommand {
        command: "/speech",
        description: "View speech configuration and status",
    },
    SlashCommand {
        command: "/listen",
        description: "Start/stop continuous live transcription",
    },
    SlashCommand {
        command: "/microphone",
        description: "List microphones or select one (/microphone select <index>)",
    },
    SlashCommand {
        command: "/screenshot",
        description: "Show screenshot capture + vision model status",
    },
    SlashCommand {
        command: "/config",
        description: "Show settings (/config set <key> <value> to edit)",
    },
    SlashCommand {
        command: "/loops",
        description: "List/toggle background loops",
    },
    SlashCommand {
        command: "/help",
        description: "Show all commands",
    },
    SlashCommand {
        command: "/close",
        description: "Close CLI (keep tray/runtime alive)",
    },
    SlashCommand {
        command: "/shutdown",
        description: "Shut down Sena completely",
    },
];

struct SlashDropdown {
    filtered: Vec<usize>,
    selected: usize,
}

struct PendingTransparencyQuery {
    query: TransparencyQuery,
    started_at: Instant,
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
    /// Close the CLI session, but keep runtime and tray alive.
    Close,
    /// Request full app shutdown (runtime and tray).
    Shutdown,
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
    /// The currently pending transparency query, if any.
    pending_transparency: Option<PendingTransparencyQuery>,
    /// Pending model directory input flag.
    pending_model_dir_input: bool,
    /// Runtime reference for config access.
    runtime: Arc<Runtime>,
    /// Shell-local voice UX state (does not persist config).
    voice_enabled: bool,
    /// Last emitted download-progress bucket (0..=10) keyed by request ID.
    download_progress: HashMap<u64, u64>,
    /// True while continuous listen mode is active in this CLI session.
    listen_mode_active: bool,
    /// Session ID of the currently active listen session (0 when inactive).
    listen_session_id: u64,
    /// Loop states: loop_name → enabled
    loop_states: HashMap<String, bool>,
}

impl Shell {
    /// Create a new Shell instance.
    fn new(runtime: Arc<Runtime>) -> Self {
        let voice_enabled = runtime.config.speech_enabled;
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

        // Pre-populate actor health — all actors are confirmed Ready by the time
        // Shell::new() is called (boot_ready() waits for ActorReady from every actor
        // before returning the Runtime). Only transitions to Failed on ActorFailed bus events.
        let mut actor_health = HashMap::new();
        actor_health.insert("Platform", ActorStatus::Ready);
        actor_health.insert("Inference", ActorStatus::Ready);
        actor_health.insert("CTP", ActorStatus::Ready);
        actor_health.insert("Memory", ActorStatus::Ready);
        // Initialize loop states - all loops enabled by default
        let mut loop_states = HashMap::new();
        loop_states.insert("ctp".to_string(), true);
        loop_states.insert("memory_consolidation".to_string(), true);
        loop_states.insert("platform_polling".to_string(), true);
        loop_states.insert("screen_capture".to_string(), true);
        loop_states.insert("speech".to_string(), true);

        actor_health.insert("Soul", ActorStatus::Ready);

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
            loop_states,
            transparency_loading: false,
            pending_transparency: None,
            pending_model_dir_input: false,
            runtime,
            voice_enabled,
            download_progress: HashMap::new(),
            listen_mode_active: false,
            listen_session_id: 0,
        }
    }

    /// Render the TUI.
    fn render(&self, frame: &mut ratatui::Frame) {
        // ── Vertical layout: header / body / input ────────────────────────────
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header bar
                Constraint::Min(1),    // Body area (conversation + sidebar)
                Constraint::Length(5), // Input area (border + 3 content lines)
            ])
            .split(frame.area());

        // ── Body: conversation (60%) + sidebar (40%) ──────────────────────────
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(main_chunks[1]);

        self.render_header(frame, main_chunks[0]);
        self.render_conversation(frame, body_chunks[0]);
        self.render_sidebar(frame, body_chunks[1]);
        self.render_input(frame, main_chunks[2]);

        // ── Overlays ──────────────────────────────────────────────────────────
        if let Some(popup) = &self.model_popup {
            model_selector::render_popup(popup, frame);
        }
        if self.model_popup.is_none() && !self.actors_popup_visible {
            self.render_slash_dropdown(frame, main_chunks[2]);
        }
        if self.actors_popup_visible {
            self.render_actors_popup(frame);
        }
    }

    /// Render the compact header bar.
    fn render_header(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let elapsed = self.stats.elapsed_formatted();
        let voice_indicator = if !self.runtime.config.speech_enabled {
            Span::styled("voice: off", Style::default().fg(Color::DarkGray))
        } else if self.voice_enabled {
            Span::styled("voice: \u{25cf} on", Style::default().fg(Color::Green))
        } else {
            Span::styled("voice: \u{25cb} off", Style::default().fg(Color::DarkGray))
        };

        let header_line = Line::from(vec![
            Span::styled(
                " \u{25c6} SENA",
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  \u{2014}  local-first ambient AI",
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("    "),
            Span::styled("\u{25b8} ", Style::default().fg(Color::DarkGray)),
            Span::styled(elapsed, Style::default().fg(Color::White)),
            Span::styled("  \u{007c}  ", Style::default().fg(Color::DarkGray)),
            voice_indicator,
        ]);

        let header = Paragraph::new(header_line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta)),
        );

        frame.render_widget(header, area);
    }

    /// Render the conversation log inside a titled bordered frame.
    fn render_conversation(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let mut lines = Vec::new();

        for msg in &self.messages {
            match msg.role {
                MessageRole::User => {
                    lines.push(Line::from(vec![
                        Span::styled(
                            "\u{25b8} ",
                            Style::default()
                                .fg(Color::LightMagenta)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(&msg.text, Style::default().fg(Color::White)),
                    ]));
                    lines.push(Line::from("")); // spacing
                }
                MessageRole::Sena => {
                    for line in msg.text.lines() {
                        lines.push(Line::from(Span::styled(
                            line,
                            Style::default().fg(Color::Green),
                        )));
                    }
                    lines.push(Line::from("")); // spacing
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
                            "\u{26a0} ",
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
        let inner_height = area.height.saturating_sub(2) as usize; // subtract borders
        let scroll = if self.scroll_offset == 0 {
            total_lines.saturating_sub(inner_height)
        } else {
            total_lines.saturating_sub(inner_height + self.scroll_offset)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta))
            .title(Span::styled(
                " Conversation ",
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            ));

        let paragraph = Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));

        frame.render_widget(paragraph, area);
    }

    /// Render the input area — bordered frame with prompt, status, and key hints.
    fn render_input(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        // Border color reflects current state
        let border_color = if self.waiting_for_inference || self.transparency_loading {
            Color::Yellow
        } else {
            Color::Magenta
        };

        // ── Line 1: prompt ────────────────────────────────────────────────────
        let prompt_line = Line::from(vec![
            Span::styled(
                " sena ",
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("\u{203a} ", Style::default().fg(Color::DarkGray)),
            Span::raw(&self.editor.input),
            Span::styled("\u{258c}", Style::default().fg(Color::LightMagenta)),
        ]);

        // ── Line 2: status ────────────────────────────────────────────────────
        let status_line = if let Some(first_press) = self.ctrl_c_first_press {
            if first_press.elapsed() < Duration::from_secs(3) {
                Line::from(Span::styled(
                    " Press Ctrl+C again to exit",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                self.status_line_normal()
            }
        } else if self.transparency_loading {
            Line::from(Span::styled(
                " \u{22ef} Loading...",
                Style::default().fg(Color::Yellow),
            ))
        } else if self.waiting_for_inference {
            Line::from(Span::styled(
                " \u{22ef} Thinking...",
                Style::default().fg(Color::Yellow),
            ))
        } else {
            self.status_line_normal()
        };

        // ── Line 3: keyboard hints ────────────────────────────────────────────
        let hints_line = Line::from(Span::styled(
            " Tab:\u{2508}complete   \u{2191}\u{2193}:\u{2508}history   Ctrl+Y:\u{2508}copy   /help:\u{2508}commands",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(
                " Input ",
                Style::default()
                    .fg(border_color)
                    .add_modifier(Modifier::BOLD),
            ));

        let paragraph = Paragraph::new(vec![prompt_line, status_line, hints_line]).block(block);
        frame.render_widget(paragraph, area);
    }

    /// Generate the normal status line.
    fn status_line_normal(&self) -> Line<'static> {
        let model_part = self
            .current_model
            .as_deref()
            .map(|m| {
                let truncated = if m.len() > 22 { &m[..22] } else { m };
                format!(" \u{25b8} {}", truncated)
            })
            .unwrap_or_default();
        let verbose_part = if self.verbose { "  [verbose]" } else { "" };
        let text = format!(" Ready{}{}", model_part, verbose_part);
        Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
    }

    /// Render the right-side status sidebar.
    fn render_sidebar(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let mut lines: Vec<Line> = Vec::new();

        // ── Model ─────────────────────────────────────────────────────────────
        lines.push(Line::from(Span::styled(
            " Model",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )));
        let model = self.current_model.as_deref().unwrap_or("(selecting...)");
        let inner_w = area.width.saturating_sub(4) as usize;
        let display_model = if model.len() > inner_w {
            &model[..inner_w]
        } else {
            model
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", display_model),
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(""));

        // ── Session stats ─────────────────────────────────────────────────────
        lines.push(Line::from(Span::styled(
            " Session",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                self.stats.messages_sent.to_string(),
                Style::default().fg(Color::White),
            ),
            Span::styled(" messages  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.stats.tokens_received.to_string(),
                Style::default().fg(Color::White),
            ),
            Span::styled(" tokens", Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(""));

        // ── Actors ────────────────────────────────────────────────────────────
        lines.push(Line::from(Span::styled(
            " Actors",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )));
        let actor_names = ["Platform", "Inference", "CTP", "Memory", "Soul"];
        for name in &actor_names {
            let status = self
                .actor_health
                .get(name)
                .unwrap_or(&ActorStatus::Starting);
            let (dot, color, label) = match status {
                ActorStatus::Ready => ("\u{25cf}", Color::Green, "Ready"),
                ActorStatus::Starting => ("\u{25cb}", Color::Yellow, "Starting"),
                ActorStatus::Failed(_) => ("\u{00d7}", Color::Red, "Failed"),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", dot), Style::default().fg(color)),
                Span::styled(format!("{:<10}", name), Style::default().fg(Color::White)),
                Span::styled(label, Style::default().fg(color)),
            ]));
        }
        lines.push(Line::from(""));

        // ── Voice ─────────────────────────────────────────────────────────────
        if self.runtime.config.speech_enabled {
            lines.push(Line::from(Span::styled(
                " Voice",
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            )));
            let (dot, color, label) = if self.voice_enabled {
                ("\u{25cf}", Color::Green, "ON  (listening)")
            } else {
                ("\u{25cb}", Color::DarkGray, "OFF")
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", dot), Style::default().fg(color)),
                Span::styled(label, Style::default().fg(color)),
            ]));
            lines.push(Line::from(""));
        }

        // ── Verbose mode indicator ────────────────────────────────────────────
        if self.verbose {
            lines.push(Line::from(Span::styled(
                "  [verbose mode on]",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )));
        }

        // ── Loops ─────────────────────────────────────────────────────────────
        lines.push(Line::from(Span::styled(
            " Loops",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        )));
        let loop_order = [
            ("ctp", "CTP"),
            ("memory_consolidation", "Memory Consolidation"),
            ("platform_polling", "Platform Polling"),
            ("screen_capture", "Screen Capture"),
            ("speech", "Speech"),
        ];
        for (loop_name, display_name) in &loop_order {
            let enabled = self.loop_states.get(*loop_name).copied().unwrap_or(true);
            let (dot, color) = if enabled {
                ("\u{25cf}", Color::Green)
            } else {
                ("\u{25cf}", Color::Red)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {} ", dot), Style::default().fg(color)),
                Span::styled(*display_name, Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::from(""));

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta))
            .title(Span::styled(
                " Status ",
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            ));

        let paragraph = Paragraph::new(Text::from(lines)).block(block);
        frame.render_widget(paragraph, area);
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
                    .border_style(Style::default().fg(Color::LightMagenta))
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
                            .fg(Color::LightMagenta)
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
                    .border_style(Style::default().fg(Color::LightMagenta))
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
    async fn handle_bus_event(&mut self, event: Event) {
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
            }) if self.pending_inference_id == Some(request_id) => {
                self.pending_inference_id = None;
                self.waiting_for_inference = false;
                if text.trim().is_empty() {
                    self.add_message(
                        MessageRole::Warning,
                        "Model returned empty response".to_string(),
                    );
                } else {
                    self.add_message(MessageRole::Sena, text);
                    self.stats.tokens_received += token_count;
                }
            }
            Event::Inference(InferenceEvent::InferenceFailed { request_id, reason })
                if self.pending_inference_id == Some(request_id) =>
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
                self.current_model = Some(name);
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
            Event::System(bus::events::SystemEvent::ActorFailed(ref info)) => {
                tracing::error!(
                    "cli: actor '{}' failed: {}",
                    info.actor_name,
                    info.error_msg
                );
                self.add_message(
                    MessageRole::Warning,
                    format!("Actor '{}' failed: {}", info.actor_name, info.error_msg),
                );
                if let Some(status) = self.actor_health.get_mut(info.actor_name) {
                    *status = ActorStatus::Failed(info.error_msg.clone());
                }
            }
            Event::System(bus::events::SystemEvent::MemoryThresholdExceeded {
                current_mb,
                limit_mb,
            }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!(
                        "Memory usage {}MB exceeds limit {}MB — consider restarting",
                        current_mb, limit_mb
                    ),
                );
            }
            // ── Memory subsystem notices ─────────────────────────────────────
            Event::Memory(bus::MemoryEvent::ConflictDetected(conflict)) => {
                self.add_message(
                    MessageRole::System,
                    format!("Memory conflict detected: {}", conflict.description),
                );
            }
            // ── Transparency query responses (async, non-blocking) ─────────────
            Event::Transparency(TransparencyEvent::ObservationResponded(resp)) => {
                self.pending_transparency = None;
                self.transparency_loading = false;
                self.add_message(
                    MessageRole::System,
                    "\u{2501}\u{2501}  Current Observation".to_string(),
                );
                self.add_message(MessageRole::Sena, format_observation_tui(&resp));
            }
            Event::Transparency(TransparencyEvent::MemoryResponded(resp)) => {
                self.pending_transparency = None;
                self.transparency_loading = false;
                self.add_message(MessageRole::System, "\u{2501}\u{2501}  Memory".to_string());
                self.add_message(MessageRole::Sena, format_memory_tui(&resp));
            }
            Event::Transparency(TransparencyEvent::InferenceExplanationResponded(resp)) => {
                self.pending_transparency = None;
                self.transparency_loading = false;
                self.add_message(
                    MessageRole::System,
                    "\u{2501}\u{2501}  Last Inference".to_string(),
                );
                self.add_message(MessageRole::Sena, format_explanation_tui(&resp));
            }
            Event::System(bus::events::SystemEvent::TrayMenuClicked(item)) => match item {
                bus::events::TrayMenuItem::ShowStatus => {
                    let model = self.current_model.as_deref().unwrap_or("(unknown)");
                    self.add_message(
                        MessageRole::System,
                        format!(
                            "Status: ready • model={} • messages={} • tokens={}",
                            model, self.stats.messages_sent, self.stats.tokens_received
                        ),
                    );
                }
                bus::events::TrayMenuItem::ShowLastThought => {
                    if let Some(last_text) = self
                        .messages
                        .iter()
                        .rev()
                        .find(|m| matches!(m.role, MessageRole::Sena))
                        .map(|m| m.text.clone())
                    {
                        self.add_message(MessageRole::System, "━━  Last Thought".to_string());
                        self.add_message(MessageRole::Sena, last_text);
                    } else {
                        self.add_message(
                            MessageRole::Warning,
                            "No thoughts yet in this session.".to_string(),
                        );
                    }
                }
                bus::events::TrayMenuItem::OpenCli
                | bus::events::TrayMenuItem::ViewLogs
                | bus::events::TrayMenuItem::Quit => {}
            },
            Event::System(bus::events::SystemEvent::CliAttachRequested) => {
                self.add_message(MessageRole::System, "CLI session already open.".to_string());
            }
            Event::System(bus::events::SystemEvent::ConfigReloaded) => {
                self.add_message(MessageRole::System, "Config reloaded.".to_string());
            }
            // In CLI-only mode (transitional, per §8.1), the supervisor isn't running,
            // so the shell handles ConfigSetRequested directly. In daemon+IPC mode (Phase 6+),
            // the supervisor handles this and the shell only renders the response events.
            Event::System(bus::events::SystemEvent::ConfigSetRequested { key, value }) => {
                match runtime::config::apply_config_set(&key, &value).await {
                    Ok(()) => {
                        tracing::info!("config set: {} = {}", key, value);
                        let _ = self
                            .bus
                            .broadcast(Event::System(bus::events::SystemEvent::ConfigReloaded))
                            .await;
                    }
                    Err(reason) => {
                        self.add_message(
                            MessageRole::Warning,
                            format!("Config set '{}' failed: {}", key, reason),
                        );
                    }
                }
            }
            Event::System(bus::events::SystemEvent::ConfigSetFailed { key, reason }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Config set '{}' failed: {}", key, reason),
                );
            }
            Event::System(bus::events::SystemEvent::TokenBudgetAutoTuned {
                old_max_tokens,
                new_max_tokens,
                p95_tokens,
            }) => {
                self.add_message(
                    MessageRole::System,
                    format!(
                        "[auto-tune] Token budget adjusted: {} \u{2192} {} (P95 usage: {}). Use /config set auto_tune_tokens false to disable.",
                        old_max_tokens, new_max_tokens, p95_tokens
                    ),
                );
            }
            Event::Speech(SpeechEvent::TranscriptionCompleted {
                text,
                confidence: _,
                request_id,
            }) => {
                if self.voice_enabled {
                    self.send_chat_with_request(
                        text,
                        request_id,
                        Priority::Normal,
                        Some("[voice] "),
                    )
                    .await;
                }
            }
            Event::Speech(SpeechEvent::TranscriptionFailed { reason, .. }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Voice transcription failed: {}", reason),
                );
            }
            Event::Download(DownloadEvent::Started {
                model_name,
                total_bytes,
                request_id,
            }) => {
                let mb = total_bytes / (1024 * 1024);
                self.add_message(
                    MessageRole::System,
                    format!("Downloading {} ({} MB)...", model_name, mb),
                );
                self.download_progress.insert(request_id, 0);
            }
            Event::Download(DownloadEvent::Progress {
                model_name,
                bytes_downloaded,
                total_bytes,
                request_id,
            }) => {
                let pct = if total_bytes > 0 {
                    bytes_downloaded * 100 / total_bytes
                } else {
                    0
                };
                let bucket = pct / 10;
                let prev = self
                    .download_progress
                    .get(&request_id)
                    .copied()
                    .unwrap_or(0);
                if bucket > prev {
                    self.add_message(MessageRole::System, format!("  {} … {}%", model_name, pct));
                    self.download_progress.insert(request_id, bucket);
                }
            }
            Event::Download(DownloadEvent::Completed {
                model_name,
                cached_path: _,
                request_id,
            }) => {
                self.download_progress.remove(&request_id);
                self.add_message(
                    MessageRole::System,
                    format!("Download complete: {}", model_name),
                );
            }
            Event::Download(DownloadEvent::Failed {
                model_name,
                reason,
                request_id: _,
            }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Download failed: {} — {}", model_name, reason),
                );
            }
            Event::Speech(SpeechEvent::WakewordDetected { confidence }) => {
                if !self.listen_mode_active {
                    self.add_message(
                        MessageRole::System,
                        format!("[speech] Wakeword detected (confidence: {:.2})", confidence),
                    );
                }
            }
            Event::Speech(SpeechEvent::SpeechOnboardingStarted) => {
                self.add_message(
                    MessageRole::System,
                    "[speech] Setting up speech subsystem...".to_string(),
                );
            }
            Event::Speech(SpeechEvent::SpeechOnboardingCompleted { models_downloaded }) => {
                self.add_message(
                    MessageRole::System,
                    format!(
                        "[speech] Speech ready! Models: {}",
                        models_downloaded.join(", ")
                    ),
                );
            }
            Event::Speech(SpeechEvent::SpeechOnboardingFailed { reason, .. }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("[speech] Speech setup failed: {}", reason),
                );
            }
            Event::Speech(SpeechEvent::SpeechOutputCompleted { .. }) => {
                self.add_message(
                    MessageRole::System,
                    "[speech] TTS playback complete".to_string(),
                );
            }
            Event::Speech(SpeechEvent::SpeechFailed { reason, .. }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("[speech] Speech failed: {}", reason),
                );
            }
            Event::Speech(SpeechEvent::ListenModeTranscription {
                text,
                is_final,
                confidence,
                session_id,
            }) if session_id == self.listen_session_id => {
                if confidence < 0.6 {
                    self.add_message(
                        MessageRole::Warning,
                        format!("[listen ~{:.0}%] {}", confidence * 100.0, text),
                    );
                } else if is_final {
                    self.add_message(MessageRole::Sena, text);
                } else {
                    self.add_message(MessageRole::System, format!("[\u{2026}] {}", text));
                }
            }
            Event::System(bus::events::SystemEvent::LoopStatusChanged { loop_name, enabled }) => {
                self.loop_states.insert(loop_name.clone(), enabled);
                if self.verbose {
                    let status = if enabled { "enabled" } else { "disabled" };
                    self.add_message(
                        MessageRole::System,
                        format!("[verbose] Loop {}: {}", loop_name, status),
                    );
                }
            }
            Event::Speech(SpeechEvent::ListenModeStopped { session_id })
                if session_id == self.listen_session_id =>
            {
                self.listen_mode_active = false;
                self.add_message(
                    MessageRole::System,
                    "\u{1f3a4} Listen mode stopped.".to_string(),
                );
            }
            other if self.verbose => {
                if let Some(msg) = verbose_format(&other) {
                    self.add_message(MessageRole::System, msg);
                }
            }
            _ => {}
        }
    }

    /// Dispatch user input (command or chat).
    async fn dispatch_line(&mut self, line: String) -> DispatchResult {
        // Handle pending model directory input
        if self.pending_model_dir_input {
            self.pending_model_dir_input = false;
            return self.handle_model_dir_input(line).await;
        }

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
                let models_dir = self.current_models_dir();
                match models_dir
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("Could not resolve models directory"))
                    .and_then(model_selector::discover_models_at)
                {
                    Ok(models) => {
                        self.model_popup = Some(model_selector::ModelSelectorPopup::new(models));
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
            "/voice" => {
                if !self.runtime.config.speech_enabled {
                    self.add_message(
                        MessageRole::Warning,
                        "Voice is unavailable because speech is disabled in config.".to_string(),
                    );
                    return DispatchResult::Continue;
                }

                self.voice_enabled = !self.voice_enabled;
                let state = if self.voice_enabled { "ON" } else { "OFF" };
                self.add_message(MessageRole::System, format!("VOICE: {}", state));
                self.add_message(
                    MessageRole::System,
                    "Voice input toggled for this CLI session; persistent runtime speech settings remain in config.".to_string(),
                );
                DispatchResult::Continue
            }
            "/speech" => {
                self.show_speech_config();
                DispatchResult::Continue
            }
            "/listen" => {
                if self.listen_mode_active {
                    // Toggle off — stop the active session.
                    let session_id = self.listen_session_id;
                    self.listen_mode_active = false;
                    if let Err(e) = self
                        .bus
                        .broadcast(Event::Speech(SpeechEvent::ListenModeStopRequested {
                            session_id,
                        }))
                        .await
                    {
                        self.add_message(
                            MessageRole::Warning,
                            format!("[listen] Failed to send stop: {}", e),
                        );
                    }
                    self.add_message(
                        MessageRole::System,
                        "\u{1f3a4} Stopping listen mode...".to_string(),
                    );
                } else {
                    if !self.runtime.config.speech_enabled {
                        self.add_message(
                            MessageRole::Warning,
                            "Speech must be enabled for /listen. Use /config set speech_enabled true".to_string(),
                        );
                        return DispatchResult::Continue;
                    }
                    // Derive a monotonically increasing session ID from message count.
                    self.listen_session_id = self.stats.messages_sent as u64
                        + std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                    self.listen_mode_active = true;
                    let session_id = self.listen_session_id;
                    if let Err(e) = self
                        .bus
                        .broadcast(Event::Speech(SpeechEvent::ListenModeRequested {
                            session_id,
                        }))
                        .await
                    {
                        self.listen_mode_active = false;
                        self.add_message(
                            MessageRole::Warning,
                            format!("[listen] Failed to start: {}", e),
                        );
                        return DispatchResult::Continue;
                    }
                    self.add_message(
                        MessageRole::System,
                        "\u{1f3a4} Listen mode started — type /listen again to stop.".to_string(),
                    );
                }
                DispatchResult::Continue
            }
            "/microphone" => {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.get(1) == Some(&"select") {
                    // /microphone select <index>
                    let idx_str = parts.get(2).copied().unwrap_or("");
                    match idx_str.parse::<usize>() {
                        Err(_) => {
                            self.add_message(
                                MessageRole::Warning,
                                "Usage: /microphone select <index>  (see /microphone for index list)".to_string(),
                            );
                        }
                        Ok(idx) => {
                            let devices = runtime::list_input_devices();
                            if let Some((_, name)) = devices.into_iter().find(|(i, _)| *i == idx) {
                                // Send ConfigSetRequested for microphone_device
                                let value = if idx == 0 {
                                    String::new() // empty value clears device (system default)
                                } else {
                                    name.clone()
                                };

                                if let Err(e) = self
                                    .bus
                                    .broadcast(Event::System(
                                        bus::events::SystemEvent::ConfigSetRequested {
                                            key: "microphone_device".to_string(),
                                            value,
                                        },
                                    ))
                                    .await
                                {
                                    self.add_message(
                                        MessageRole::Warning,
                                        format!("Failed to send config change: {}", e),
                                    );
                                } else {
                                    self.add_message(
                                        MessageRole::System,
                                        format!(
                                            "\u{1f3a4} Microphone set to: {}{}",
                                            name,
                                            if idx == 0 { " (system default)" } else { "" }
                                        ),
                                    );
                                    self.add_message(
                                        MessageRole::System,
                                        "Restart Sena for the new microphone to take effect."
                                            .to_string(),
                                    );
                                }
                            } else {
                                self.add_message(
                                    MessageRole::Warning,
                                    format!(
                                        "No device at index {}. Run /microphone to list devices.",
                                        idx
                                    ),
                                );
                            }
                        }
                    }
                } else {
                    // /microphone — list available devices
                    let devices = runtime::list_input_devices();
                    let current = self
                        .runtime
                        .config
                        .microphone_device
                        .as_deref()
                        .unwrap_or("(system default)");
                    self.add_message(
                        MessageRole::System,
                        format!(
                            "\u{2501}\u{2501}  Available Microphones  (current: {})",
                            current
                        ),
                    );
                    if devices.is_empty() {
                        self.add_message(
                            MessageRole::Warning,
                            "No input devices found.".to_string(),
                        );
                    } else {
                        for (idx, name) in &devices {
                            self.add_message(MessageRole::System, format!("  [{}]  {}", idx, name));
                        }
                        self.add_message(
                            MessageRole::System,
                            "Use /microphone select <index> to switch. Index 0 = system default."
                                .to_string(),
                        );
                    }
                }
                DispatchResult::Continue
            }
            "/screenshot" => {
                let capture_status = if self.runtime.config.screen_capture_enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                let platform_support = if cfg!(target_os = "windows") {
                    "supported"
                } else {
                    "not implemented"
                };
                let frame_ready = self
                    .runtime
                    .vision_frame_store
                    .lock()
                    .map(|frame| if frame.is_some() { "yes" } else { "no" })
                    .unwrap_or("unknown");
                let active_model = self.current_model.as_deref().unwrap_or("unknown");
                let vision_status = match self.current_model.as_deref() {
                    Some(model_name) => {
                        if is_vision_capable_model(model_name) {
                            "yes"
                        } else {
                            "no"
                        }
                    }
                    None => "unknown",
                };

                self.add_message(
                    MessageRole::System,
                    format!(
                        "Screenshot capture: {} | Platform: {} | Frame ready: {} | Active model: {} | Vision capable: {}",
                        capture_status, platform_support, frame_ready, active_model, vision_status
                    ),
                );
                self.add_message(
                    MessageRole::System,
                    "Privacy: screenshots are in-memory only and not persisted. Availability depends on platform support.".to_string(),
                );
                DispatchResult::Continue
            }
            "/config" => {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.get(1) == Some(&"set") {
                    let key = parts.get(2).copied().unwrap_or("");
                    let value = if parts.len() > 3 {
                        parts[3..].join(" ")
                    } else {
                        String::new()
                    };
                    self.set_config_value(key, &value).await;
                } else {
                    self.show_config().await;
                }
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
            "/copy" => {
                self.copy_last_response();
                DispatchResult::Continue
            }
            _ if exit_command_result(cmd).is_some() => match exit_command_result(cmd) {
                Some(result) => result,
                None => DispatchResult::Continue,
            },
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
        tracing::info!("cli: firing transparency query: {:?}", query);
        self.add_message(MessageRole::System, loading_msg.to_string());
        self.transparency_loading = true;
        if let Err(e) = self
            .bus
            .broadcast(Event::Transparency(BusTransparencyEvent::QueryRequested(
                query.clone(),
            )))
            .await
        {
            self.pending_transparency = None;
            self.transparency_loading = false;
            self.add_message(MessageRole::Warning, format!("Failed to send query: {}", e));
        } else {
            self.pending_transparency = Some(PendingTransparencyQuery {
                query,
                started_at: Instant::now(),
            });
        }
    }

    fn handle_transparency_timeout(&mut self) {
        let Some(pending) = &self.pending_transparency else {
            return;
        };

        if pending.started_at.elapsed() < TRANSPARENCY_REQUEST_TIMEOUT {
            return;
        }

        let message = transparency_timeout_message(&pending.query);
        self.pending_transparency = None;
        self.transparency_loading = false;
        self.add_message(MessageRole::Warning, message);
    }

    /// Send a chat message to the inference actor.
    async fn send_chat(&mut self, prompt: String) {
        // Generate request ID
        let request_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);

        self.send_chat_with_request(prompt, request_id, Priority::High, None)
            .await;
    }

    async fn send_chat_with_request(
        &mut self,
        prompt: String,
        request_id: u64,
        priority: Priority,
        user_prefix: Option<&str>,
    ) {
        let displayed_prompt = match user_prefix {
            Some(prefix) => format!("{}{}", prefix, prompt),
            None => prompt.clone(),
        };

        self.add_message(MessageRole::User, displayed_prompt);

        self.add_message(MessageRole::System, "Thinking...".to_string());
        self.waiting_for_inference = true;
        self.pending_inference_id = Some(request_id);
        self.stats.messages_sent += 1;

        tracing::info!(
            "cli: dispatching chat request_id={} prompt_len={}",
            request_id,
            prompt.len()
        );

        if let Err(e) = self
            .bus
            .send_directed(
                "inference",
                Event::Inference(InferenceEvent::InferenceRequested {
                    prompt,
                    priority,
                    request_id,
                    source: bus::InferenceSource::UserText,
                }),
            )
            .await
        {
            tracing::error!("cli: directed send to inference actor failed: {}", e);
            self.waiting_for_inference = false;
            self.pending_inference_id = None;
            self.add_message(
                MessageRole::Warning,
                format!("Could not reach inference actor: {}", e),
            );
        } else {
            tracing::info!(
                "cli: chat request_id={} dispatched to inference actor",
                request_id
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
            "/voice                 Toggle voice input in this CLI session".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/speech                View speech configuration and status".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/listen                Start/stop continuous live transcription".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/microphone            List microphones or select one (/microphone select <index>)"
                .to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/loops                 List all background loops and their status".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/loops <name>          Toggle a loop on/off".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/loops <name> on|off   Explicitly enable or disable a loop".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/help                  Show this message".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/close or /quit        Close the CLI session".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/shutdown              Shut down Sena completely".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "/copy                  Copy last response to clipboard".to_string(),
        );
        self.add_message(MessageRole::System, "".to_string());
        self.add_message(MessageRole::System, "━━  Keyboard Shortcuts".to_string());
        self.add_message(
            MessageRole::System,
            "Ctrl+Y                 Copy last response to clipboard".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "Ctrl+Shift+C           Copy last response to clipboard".to_string(),
        );
        self.add_message(MessageRole::System, "".to_string());
        self.add_message(
            MessageRole::System,
            "Type any message to chat with the model.".to_string(),
        );
    }

    /// Show config file path and current settings (reads directly from config file).
    ///
    /// Displays ALL settings in TOML format so new fields are never silently omitted.
    async fn show_config(&mut self) {
        let config_path = runtime::config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(unavailable)".to_string());

        // Read fresh config from file — not the boot-time snapshot.
        let config = match runtime::config::load_or_create_config().await {
            Ok(c) => c,
            Err(e) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Could not read config file: {}", e),
                );
                return;
            }
        };

        self.add_message(MessageRole::System, "━━  Configuration".to_string());
        self.add_message(MessageRole::System, format!("Config file: {}", config_path));
        self.add_message(MessageRole::System, "".to_string());

        // Serialize the full config to TOML so all fields are shown dynamically.
        // New fields added to SenaConfig appear here automatically.
        match toml::to_string_pretty(&config) {
            Ok(toml_str) => {
                // Show each line as a separate message for proper TUI wrapping
                for line in toml_str.lines() {
                    // Add ⚠ markers for settings that may need careful adjustment
                    let marked = if line.starts_with("inference_max_tokens") {
                        format!("{line}  ← low values truncate responses; auto-tune planned")
                    } else if line.starts_with("inference_ctx_size") {
                        format!("{line}  ← must not exceed model training context")
                    } else if line.starts_with("ctp_trigger_sensitivity") {
                        format!("{line}  ← 0.0–1.0; lower = less proactive")
                    } else {
                        line.to_string()
                    };
                    self.add_message(MessageRole::System, marked);
                }
            }
            Err(e) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Could not serialize config: {}", e),
                );
            }
        }

        self.add_message(MessageRole::System, "".to_string());
        self.add_message(
            MessageRole::System,
            "Use /config set <key> <value> to edit. Restart actors for most changes.".to_string(),
        );
        self.add_message(
            MessageRole::System,
            "Examples: /config set speech_enabled true  |  /config set inference_max_tokens 1024"
                .to_string(),
        );
    }

    /// Update a single config field from the CLI.
    ///
    /// Broadcasts a ConfigSetRequested event to the supervisor.
    /// The response (ConfigReloaded or ConfigSetFailed) arrives asynchronously via handle_bus_event.
    async fn set_config_value(&mut self, key: &str, value: &str) {
        if let Err(e) = self
            .bus
            .broadcast(Event::System(
                bus::events::SystemEvent::ConfigSetRequested {
                    key: key.to_string(),
                    value: value.to_string(),
                },
            ))
            .await
        {
            self.add_message(
                MessageRole::Warning,
                format!("Failed to send config change: {}", e),
            );
        } else {
            self.add_message(
                MessageRole::System,
                format!("Setting {} = {}...", key, value),
            );
        }
    }

    /// Show speech configuration and status.
    fn show_speech_config(&mut self) {
        let enabled = self.runtime.config.speech_enabled;
        let model_dir = self
            .runtime
            .config
            .speech_model_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(default platform path)".to_string());
        let always_listening = self.runtime.config.voice_always_listening;
        let wakeword_enabled = self.runtime.config.wakeword_enabled;
        let tts_rate = self.runtime.config.tts_rate;
        let proactive = self.runtime.config.proactive_speech_enabled;

        self.add_message(
            MessageRole::System,
            "\u{2501}\u{2501}  Speech Configuration".to_string(),
        );
        self.add_message(
            MessageRole::System,
            format!("speech_enabled            {}", enabled),
        );
        self.add_message(
            MessageRole::System,
            format!("speech_model_dir          {}", model_dir),
        );
        self.add_message(
            MessageRole::System,
            format!("voice_always_listening    {}", always_listening),
        );
        self.add_message(
            MessageRole::System,
            format!("wakeword_enabled          {}", wakeword_enabled),
        );
        self.add_message(
            MessageRole::System,
            format!("tts_rate                  {:.1}", tts_rate),
        );
        self.add_message(
            MessageRole::System,
            format!("proactive_speech_enabled  {}", proactive),
        );
        self.add_message(
            MessageRole::System,
            format!(
                "voice_session_active      {}",
                if self.voice_enabled { "yes" } else { "no" }
            ),
        );
        self.add_message(MessageRole::System, "".to_string());
        if enabled {
            self.add_message(
                MessageRole::System,
                "Speech is enabled. Use /voice to toggle microphone input for this session."
                    .to_string(),
            );
            self.add_message(
                MessageRole::System,
                "To change persistent speech settings: edit config file and restart.".to_string(),
            );
        } else {
            self.add_message(
                MessageRole::System,
                "Speech is disabled. To enable: set speech_enabled = true in config and restart."
                    .to_string(),
            );
        }
    }

    /// Copy the most recent Sena response to the system clipboard.
    fn copy_last_response(&mut self) {
        let last_response = self
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, MessageRole::Sena))
            .map(|m| m.text.clone());

        match last_response {
            Some(text) => match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&text)) {
                Ok(_) => {
                    self.add_message(
                        MessageRole::System,
                        "Last response copied to clipboard.".to_string(),
                    );
                }
                Err(e) => {
                    self.add_message(MessageRole::Warning, format!("Copy failed: {}", e));
                }
            },
            None => {
                self.add_message(MessageRole::System, "No response to copy yet.".to_string());
            }
        }
    }

    /// Handle model directory path input.
    async fn handle_model_dir_input(&mut self, path_str: String) -> DispatchResult {
        let previous_dir = self.current_models_dir();
        let trimmed = path_str.trim();

        if trimmed.is_empty() {
            self.add_message(
                MessageRole::System,
                "Model directory change cancelled. Keeping current directory.".to_string(),
            );
            return DispatchResult::Continue;
        }

        let path = std::path::PathBuf::from(trimmed);

        // Validate directory contains GGUF models.
        match model_selector::discover_models_at(&path) {
            Ok(models) => {
                let model_count = models.len();
                let mut config = self.runtime.config.clone();
                config.models_dir = Some(path.clone());

                match runtime::save_config(&config).await {
                    Ok(_) => {
                        self.add_message(
                            MessageRole::System,
                            format!(
                                "Model directory set to: {} ({} models found)",
                                path.display(),
                                model_count
                            ),
                        );
                        self.model_popup = Some(model_selector::ModelSelectorPopup::new(models));
                    }
                    Err(e) => {
                        self.add_message(
                            MessageRole::Warning,
                            format!("Failed to save config: {}", e),
                        );
                    }
                }
            }
            Err(e) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Model directory rejected: {}", e),
                );
                if let Some(prev) = previous_dir {
                    self.add_message(
                        MessageRole::System,
                        format!("Keeping previous model directory: {}", prev.display()),
                    );
                }
            }
        }

        DispatchResult::Continue
    }

    fn current_models_dir(&self) -> Option<PathBuf> {
        if let Some(path) = self.runtime.config.models_dir.clone() {
            return Some(path);
        }

        runtime::ollama_models_dir().ok()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DispatchResult {
    Continue,
    Close,
    Shutdown,
}

fn exit_command_result(command: &str) -> Option<DispatchResult> {
    match command {
        "/close" | "/quit" | "/exit" | "/q" => Some(DispatchResult::Close),
        "/shutdown" => Some(DispatchResult::Shutdown),
        _ => None,
    }
}

fn is_vision_capable_model(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("llava")
        || n.contains("bakllava")
        || n.contains("vision")
        || n.contains("minicpm-v")
        || n.contains("phi-3-v")
        || n.contains("phi3-v")
        || n.contains("moondream")
        || n.contains("idefics")
        || n.contains("cogvlm")
}

/// Run the interactive shell. Returns the exit reason for the restart loop.
pub async fn run(runtime: Arc<Runtime>) -> Result<ShellExitReason> {
    // ── Enter raw mode and alternate screen ───────────────────────────────────
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let _guard = TerminalGuard; // Ensures cleanup on drop

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // ── Initialize shell state ────────────────────────────────────────────────
    let mut shell = Shell::new(Arc::clone(&runtime));

    // ── Ctrl-C shutdown watch ─────────────────────────────────────────────────
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });

    // ── Bus subscriber for events ─────────────────────────────────────────────
    let mut bus_rx = runtime.bus.subscribe_broadcast();

    let mut exit_reason = ShellExitReason::Close;

    // ── Main event loop ───────────────────────────────────────────────────────
    loop {
        // Render the current state
        if let Err(e) = terminal.draw(|f| shell.render(f)) {
            shell.add_message(MessageRole::Warning, format!("Display error: {}", e));
            // Try to continue — next frame may succeed
        }

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
                match bcast {
                    Ok(ev) => {
                        if matches!(ev, Event::System(bus::events::SystemEvent::ShutdownSignal)) {
                            tracing::info!("cli: ShutdownSignal received on bus — exiting shell");
                            exit_reason = ShellExitReason::Shutdown;
                            break;
                        }
                        shell.handle_bus_event(ev).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::error!("cli: broadcast channel closed unexpectedly — exiting shell");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("cli: broadcast receiver lagged by {} events — some events dropped", n);
                    }
                }
            }

            // Keyboard events (poll in a non-blocking way)
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                shell.handle_transparency_timeout();

                // Poll for crossterm events
                let poll_result = event::poll(Duration::from_millis(0));
                match poll_result {
                    Ok(true) => {},
                    Ok(false) => continue,
                    Err(e) => {
                        shell.add_message(
                            MessageRole::Warning,
                            format!("Poll error: {}", e),
                        );
                        continue;
                    }
                }

                match event::read() {
                    Err(e) => {
                        shell.add_message(
                            MessageRole::Warning,
                            format!("Input error: {}", e),
                        );
                        continue;
                    }
                    Ok(event::Event::Mouse(mouse)) => {
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

                    Ok(event::Event::Key(key)) => {
                        // Filter out key release events (Windows)
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        match (key.code, key.modifiers) {
                            // Ctrl+Shift+C — copy last response to clipboard
                            (KeyCode::Char('c'), mods)
                                if mods.contains(KeyModifiers::CONTROL)
                                    && mods.contains(KeyModifiers::SHIFT) =>
                            {
                                shell.copy_last_response();
                            }

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

                            // Ctrl+Y — copy last response to clipboard
                            (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                                shell.copy_last_response();
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
                                    if popup.is_change_dir_selected() {
                                        // Prompt for directory path
                                        shell.pending_model_dir_input = true;
                                        shell.add_message(
                                            MessageRole::System,
                                            "Enter the full path to your model directory (Enter on empty input to cancel):".to_string(),
                                        );
                                    } else if let Some(selected) = popup.selected() {
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
                                        DispatchResult::Close => {
                                            exit_reason = ShellExitReason::Close;
                                            break;
                                        }
                                        DispatchResult::Shutdown => {
                                            exit_reason = ShellExitReason::Shutdown;
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
                                        DispatchResult::Close => {
                                            exit_reason = ShellExitReason::Close;
                                            break;
                                        }
                                        DispatchResult::Shutdown => {
                                            exit_reason = ShellExitReason::Shutdown;
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

    // ── Graceful shutdown ─────────────────────────────────────────────────────
    drop(_guard); // Restore terminal
    drop(terminal); // Drop terminal before printing to stdout

    // Extract stats before dropping shell
    let stats = shell.stats.clone();

    // Drop shell to release Arc<Runtime> reference
    drop(shell);

    display::print_session_summary(&stats);

    if exit_reason == ShellExitReason::Close {
        let _ = runtime
            .bus
            .broadcast(Event::System(bus::events::SystemEvent::CliSessionClosed))
            .await;
    }

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
        out.push_str(&format!("\n  {}", empty_memory_message_tui(summary)));
    } else {
        for (i, chunk) in resp.memory_chunks.iter().enumerate() {
            let preview = if chunk.text.chars().count() > 120 {
                let truncated: String = chunk.text.chars().take(120).collect();
                format!("{}...", truncated)
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
    let request = if resp.request_context.chars().count() > 200 {
        let truncated: String = resp.request_context.chars().take(200).collect();
        format!("{}...", truncated)
    } else {
        resp.request_context.clone()
    };
    let response = if resp.response_text.chars().count() > 299 {
        let truncated: String = resp.response_text.chars().take(299).collect();
        format!("{}...", truncated)
    } else {
        resp.response_text.clone()
    };
    let mut out = format!(
        "Rounds: {}\nRequest   {request}\nResponse  {response}",
        resp.rounds_completed
    );
    if resp.working_memory_context.is_empty() {
        out.push_str("\nWorking Memory  (none used in the last completed cycle)");
    } else {
        out.push_str(&format!(
            "\nWorking Memory  {} chunks used in the last completed cycle",
            resp.working_memory_context.len()
        ));
        for (i, chunk) in resp.working_memory_context.iter().enumerate() {
            let preview = if chunk.text.chars().count() > 80 {
                let truncated: String = chunk.text.chars().take(80).collect();
                format!("{}...", truncated)
            } else {
                chunk.text.clone()
            };
            out.push_str(&format!("\n          [{}] {preview}", i + 1));
        }
    }
    out
}

fn empty_memory_message_tui(
    summary: &bus::events::transparency::SoulSummaryForTransparency,
) -> &'static str {
    if summary.inference_cycle_count == 0
        && summary.work_patterns.is_empty()
        && summary.tool_preferences.is_empty()
        && summary.interest_clusters.is_empty()
    {
        "No user memory is available yet. Sena has not retained any memories for this profile."
    } else {
        "No retrievable memory snippets are available right now."
    }
}

fn transparency_timeout_message(query: &TransparencyQuery) -> String {
    match query {
        TransparencyQuery::CurrentObservation => {
            "Observation is taking too long. Sena will keep running; try again in a moment.".to_string()
        }
        TransparencyQuery::UserMemory => {
            "Memory is taking too long to respond. No memory data was returned, but Sena is still running.".to_string()
        }
        TransparencyQuery::InferenceExplanation => {
            "Explanation is taking too long to respond. Try /explanation again after the next completed inference cycle.".to_string()
        }
        TransparencyQuery::ModelList => {
            "Model list query timed out. This should not happen; check the inference actor.".to_string()
        }
    }
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

/// Run CLI in IPC mode — connect to running daemon and render TUI.
///
/// Phase 6: The CLI connects to the daemon over IPC (Unix socket / named pipe)
/// instead of booting its own runtime. All bus events and commands flow through
/// the IPC channel.
pub async fn run_with_ipc() -> anyhow::Result<()> {
    use crate::ipc_client::IpcClient;
    use bus::{IpcPayload, LineStyle};

    crate::display::banner();
    crate::display::info("Connecting to Sena daemon...");

    // Connect to daemon IPC endpoint.
    let mut ipc_client = IpcClient::connect()
        .await
        .map_err(|e| anyhow::anyhow!("IPC connection failed: {}", e))?;

    // Send Subscribe to register for event stream.
    ipc_client
        .send(IpcPayload::Subscribe)
        .await
        .map_err(|e| anyhow::anyhow!("Subscribe failed: {}", e))?;

    // Wait for SessionReady from daemon.
    let initial_model: Option<String> = loop {
        match ipc_client.recv().await {
            Some(msg) => {
                if let IpcPayload::SessionReady { current_model, .. } = msg.payload {
                    tracing::info!("IPC session established");
                    break current_model;
                } else {
                    tracing::warn!("unexpected IPC message before SessionReady: {:?}", msg);
                }
            }
            None => {
                return Err(anyhow::anyhow!("daemon disconnected during handshake"));
            }
        }
    };

    crate::display::success("Connected to daemon.");

    // ── Enter raw mode and alternate screen ───────────────────────────────────
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let _guard = TerminalGuard; // Ensures cleanup on drop

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // ── Initialize simplified IPC shell state ─────────────────────────────────
    let mut messages: Vec<Message> = vec![
        Message::new(
            MessageRole::System,
            "Connected to Sena daemon — local-first ambient AI".to_string(),
        ),
        Message::new(
            MessageRole::System,
            "Type /help for commands, or chat freely.".to_string(),
        ),
    ];
    let mut editor = EditorState::new();
    let mut stats = SessionStats::new();
    let mut scroll_offset = 0usize;
    let mut ctrl_c_first_press: Option<Instant> = None;
    let mut slash_dropdown: Option<SlashDropdown> = None;

    // Initialize loop states - all loops enabled by default
    let mut loop_states: HashMap<String, bool> = HashMap::new();
    loop_states.insert("ctp".to_string(), true);
    loop_states.insert("memory_consolidation".to_string(), true);
    loop_states.insert("platform_polling".to_string(), true);
    loop_states.insert("screen_capture".to_string(), true);
    loop_states.insert("speech".to_string(), true);

    // Initialize actor health - all actors assumed Ready if we connected to daemon
    let mut actor_health: HashMap<&'static str, ActorStatus> = HashMap::new();
    actor_health.insert("Platform", ActorStatus::Ready);
    actor_health.insert("Inference", ActorStatus::Ready);
    actor_health.insert("CTP", ActorStatus::Ready);
    actor_health.insert("Memory", ActorStatus::Ready);
    actor_health.insert("Soul", ActorStatus::Ready);

    // Current model name — seeded from SessionReady, updated when ModelLoaded fires
    let mut current_model: Option<String> = initial_model;

    // Model selector popup state (mirrors standalone Shell behaviour)
    let mut model_popup: Option<model_selector::ModelSelectorPopup> = None;
    let mut pending_model_dir_input = false;

    // ── Main event loop ───────────────────────────────────────────────────────
    loop {
        // Render the current state.
        if let Err(e) = terminal.draw(|f| {
            render_ipc_tui(
                f,
                &messages,
                &editor,
                &stats,
                scroll_offset,
                ctrl_c_first_press,
                slash_dropdown.as_ref(),
                &loop_states,
                &actor_health,
                current_model.as_deref(),
                model_popup.as_ref(),
            )
        }) {
            tracing::error!("TUI render error: {}", e);
        }

        // Check for Ctrl+C timeout (reset if 3 seconds passed).
        if let Some(first_press) = ctrl_c_first_press {
            if first_press.elapsed() > Duration::from_secs(3) {
                ctrl_c_first_press = None;
            }
        }

        tokio::select! {
            biased;

            // IPC messages from daemon
            ipc_msg = ipc_client.recv() => {
                match ipc_msg {
                    Some(msg) => {
                        match msg.payload {
                            IpcPayload::DisplayLine { content, style } => {
                                let role = match style {
                                    LineStyle::Error => MessageRole::Warning,
                                    LineStyle::CtpThought => MessageRole::Warning,
                                    LineStyle::Inference => MessageRole::Sena,
                                    LineStyle::Success => MessageRole::Sena,
                                    LineStyle::Normal => MessageRole::System,
                                    LineStyle::Dimmed => MessageRole::System,
                                    LineStyle::SystemNotice => MessageRole::System,
                                };
                                messages.push(Message::new(role, content));
                            }
                            IpcPayload::DaemonShutdown => {
                                messages.push(Message::new(
                                    MessageRole::Warning,
                                    "Daemon is shutting down — CLI will exit.".to_string(),
                                ));
                                tokio::time::sleep(Duration::from_secs(2)).await;
                                break;
                            }
                            IpcPayload::Pong => {
                                // Keepalive — ignore
                            }
                            IpcPayload::Ack { .. } => {
                                // Command acknowledged — ignore
                            }
                            IpcPayload::Error { reason, .. } => {
                                messages.push(Message::new(MessageRole::Warning, format!("Error: {}", reason)));
                            }
                            IpcPayload::LoopStatusUpdate { loop_name, enabled } => {
                                loop_states.insert(loop_name, enabled);
                                // Trigger redraw - handled automatically by next render cycle
                            }
                            IpcPayload::ModelStatusUpdate { name } => {
                                current_model = Some(name);
                            }
                            _ => {
                                tracing::warn!("unexpected IPC payload: {:?}", msg);
                            }
                        }
                    }
                    None => {
                        messages.push(Message::new(
                            MessageRole::Warning,
                            "Daemon disconnected.".to_string(),
                        ));
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        break;
                    }
                }
            }

            // Keyboard events (poll in a non-blocking way)
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                // Poll for crossterm events
                let poll_result = event::poll(Duration::from_millis(0));
                match poll_result {
                    Ok(true) => {},
                    Ok(false) => continue,
                    Err(e) => {
                        tracing::error!("Poll error: {}", e);
                        continue;
                    }
                }

                match event::read() {
                    Err(e) => {
                        tracing::error!("Input error: {}", e);
                        continue;
                    }
                    Ok(event::Event::Mouse(mouse)) => {
                        match mouse.kind {
                            event::MouseEventKind::ScrollUp => {
                                scroll_offset = scroll_offset.saturating_add(3);
                            }
                            event::MouseEventKind::ScrollDown => {
                                scroll_offset = scroll_offset.saturating_sub(3);
                            }
                            _ => {}
                        }
                    }

                    Ok(event::Event::Key(key)) => {
                        // Filter out key release events (Windows)
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        match (key.code, key.modifiers) {
                            // Ctrl+C
                            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                if let Some(first_press) = ctrl_c_first_press {
                                    if first_press.elapsed() < Duration::from_secs(3) {
                                        break;
                                    }
                                } else {
                                    ctrl_c_first_press = Some(Instant::now());
                                }
                            }

                            // Ctrl+D
                            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                break;
                            }

                            // ── Model popup key handlers ──────────────────────────────
                            (KeyCode::Up, _) if model_popup.is_some() => {
                                if let Some(popup) = &mut model_popup {
                                    popup.prev();
                                }
                            }
                            (KeyCode::Down, _) if model_popup.is_some() => {
                                if let Some(popup) = &mut model_popup {
                                    popup.next();
                                }
                            }
                            (KeyCode::Enter, _) if model_popup.is_some() => {
                                if let Some(popup) = model_popup.take() {
                                    if popup.is_change_dir_selected() {
                                        pending_model_dir_input = true;
                                        messages.push(Message::new(
                                            MessageRole::System,
                                            "Enter the full path to your model directory (Enter on empty input to cancel):".to_string(),
                                        ));
                                    } else if let Some(selected) = popup.selected() {
                                        let model_name = selected.name.clone();
                                        let cmd = format!("/config set preferred_model {}", model_name);
                                        let _ = ipc_client.send(IpcPayload::SlashCommand { line: cmd }).await;
                                        messages.push(Message::new(
                                            MessageRole::System,
                                            format!("Selected model: {} — change takes effect after daemon restart.", model_name),
                                        ));
                                        current_model = Some(model_name);
                                    }
                                }
                            }
                            (KeyCode::Esc, _) if model_popup.is_some() => {
                                model_popup = None;
                                messages.push(Message::new(
                                    MessageRole::System,
                                    "Model selection cancelled.".to_string(),
                                ));
                            }

                            // ── Slash Dropdown Key Handlers ───────────────────────────
                            (KeyCode::Up, _) if slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                if let Some(dd) = &mut slash_dropdown {
                                    dd.prev();
                                }
                            }
                            (KeyCode::Down, _) if slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                if let Some(dd) = &mut slash_dropdown {
                                    dd.next();
                                }
                            }
                            (KeyCode::Tab, _) if slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                // Complete the command into the input
                                if let Some(cmd) = slash_dropdown.as_ref().and_then(|d| d.selected_command()) {
                                    editor.input = cmd.to_string();
                                    slash_dropdown = None;
                                }
                            }
                            (KeyCode::Enter, _) if slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                // Complete and immediately submit
                                if let Some(cmd) = slash_dropdown.as_ref().and_then(|d| d.selected_command()) {
                                    let line = cmd.to_string();
                                    editor.input.clear();
                                    slash_dropdown = None;
                                    editor.push_history(&line);

                                    if line == "/models" {
                                        // Handle /models locally even when selected from dropdown
                                        let models_dir = runtime::ollama_models_dir().ok();
                                        match models_dir
                                            .as_deref()
                                            .ok_or_else(|| anyhow::anyhow!("Could not resolve models directory"))
                                            .and_then(|p| model_selector::discover_models_at(p).map_err(|e| anyhow::anyhow!("{}", e)))
                                        {
                                            Ok(models) => {
                                                model_popup = Some(model_selector::ModelSelectorPopup::new(models));
                                            }
                                            Err(e) => {
                                                messages.push(Message::new(
                                                    MessageRole::Warning,
                                                    format!("Model discovery failed: {}", e),
                                                ));
                                            }
                                        }
                                    } else if line.starts_with('/') {
                                        let _ = ipc_client.send(IpcPayload::SlashCommand { line }).await;
                                    } else {
                                        messages.push(Message::new(MessageRole::User, line.clone()));
                                        let _ = ipc_client.send(IpcPayload::Chat { text: line }).await;
                                        stats.messages_sent += 1;
                                    }
                                }
                            }
                            (KeyCode::Esc, _) if slash_dropdown.is_some() => {
                                slash_dropdown = None;
                            }

                            // ── Normal Key Handlers ───────────────────────────────────
                            // Enter — submit line
                            (KeyCode::Enter, _) => {
                                let line = editor.input.trim().to_string();
                                editor.input.clear();
                                slash_dropdown = None;
                                if !line.is_empty() {
                                    editor.push_history(&line);

                                    // Handle pending model directory input
                                    if pending_model_dir_input {
                                        pending_model_dir_input = false;
                                        if line.is_empty() {
                                            messages.push(Message::new(
                                                MessageRole::System,
                                                "Model directory change cancelled.".to_string(),
                                            ));
                                        } else {
                                            let path = std::path::PathBuf::from(&line);
                                            match model_selector::discover_models_at(&path) {
                                                Ok(models) if !models.is_empty() => {
                                                    model_popup = Some(model_selector::ModelSelectorPopup::new(models));
                                                }
                                                Ok(_) => {
                                                    messages.push(Message::new(
                                                        MessageRole::Warning,
                                                        format!("No models found in: {}", line),
                                                    ));
                                                }
                                                Err(e) => {
                                                    messages.push(Message::new(
                                                        MessageRole::Warning,
                                                        format!("Model discovery failed: {}", e),
                                                    ));
                                                }
                                            }
                                        }
                                    } else if line == "/models" {
                                        // Handle /models locally — open interactive popup
                                        let models_dir = runtime::ollama_models_dir().ok();
                                        match models_dir
                                            .as_deref()
                                            .ok_or_else(|| anyhow::anyhow!("Could not resolve models directory"))
                                            .and_then(|p| model_selector::discover_models_at(p).map_err(|e| anyhow::anyhow!("{}", e)))
                                        {
                                            Ok(models) => {
                                                model_popup = Some(model_selector::ModelSelectorPopup::new(models));
                                            }
                                            Err(e) => {
                                                messages.push(Message::new(
                                                    MessageRole::Warning,
                                                    format!("Model discovery failed: {}", e),
                                                ));
                                            }
                                        }
                                    } else if line.starts_with('/') {
                                        // Slash command
                                        if line == "/close" || line == "/quit" {
                                            break;
                                        }
                                        let _ = ipc_client.send(IpcPayload::SlashCommand { line }).await;
                                    } else {
                                        // Chat message
                                        messages.push(Message::new(MessageRole::User, line.clone()));
                                        let _ = ipc_client.send(IpcPayload::Chat { text: line }).await;
                                        stats.messages_sent += 1;
                                    }
                                }
                            }

                            // Backspace
                            (KeyCode::Backspace, _) => {
                                editor.input.pop();
                                // Update slash dropdown after backspace
                                if editor.input.starts_with('/') {
                                    if let Some(dd) = &mut slash_dropdown {
                                        dd.update(&editor.input);
                                        if dd.is_empty() {
                                            slash_dropdown = None;
                                        }
                                    } else {
                                        let dd = SlashDropdown::from_prefix(&editor.input);
                                        if !dd.is_empty() {
                                            slash_dropdown = Some(dd);
                                        }
                                    }
                                } else {
                                    slash_dropdown = None;
                                }
                            }

                            // Arrow Up — history prev
                            (KeyCode::Up, _) if slash_dropdown.is_none() => {
                                editor.history_prev();
                            }

                            // Arrow Down — history next
                            (KeyCode::Down, _) if slash_dropdown.is_none() => {
                                editor.history_next();
                            }

                            // Page Up — scroll up
                            (KeyCode::PageUp, _) => {
                                scroll_offset = scroll_offset.saturating_add(10);
                            }

                            // Page Down — scroll down
                            (KeyCode::PageDown, _) => {
                                scroll_offset = scroll_offset.saturating_sub(10);
                            }

                            // Escape — clear input and close dropdown
                            (KeyCode::Esc, _) => {
                                editor.input.clear();
                                slash_dropdown = None;
                            }

                            // Regular character
                            (KeyCode::Char(c), mods) if !mods.contains(KeyModifiers::CONTROL) && !mods.contains(KeyModifiers::ALT) => {
                                editor.input.push(c);
                                // Reset Ctrl+C first press if user is typing
                                ctrl_c_first_press = None;
                                // Update slash dropdown on every character typed
                                if editor.input.starts_with('/') {
                                    if let Some(dd) = &mut slash_dropdown {
                                        dd.update(&editor.input);
                                        if dd.is_empty() {
                                            slash_dropdown = None;
                                        }
                                    } else {
                                        let dd = SlashDropdown::from_prefix(&editor.input);
                                        if !dd.is_empty() {
                                            slash_dropdown = Some(dd);
                                        }
                                    }
                                } else {
                                    slash_dropdown = None;
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

    // ── Graceful shutdown ─────────────────────────────────────────────────────
    drop(_guard); // Restore terminal
    drop(terminal); // Drop terminal before printing to stdout

    display::print_session_summary(&stats);

    Ok(())
}

/// Simplified TUI rendering for IPC mode.
#[allow(clippy::too_many_arguments)]
fn render_ipc_tui(
    frame: &mut ratatui::Frame,
    messages: &[Message],
    editor: &EditorState,
    stats: &SessionStats,
    scroll_offset: usize,
    ctrl_c_first_press: Option<Instant>,
    slash_dropdown: Option<&SlashDropdown>,
    loop_states: &HashMap<String, bool>,
    actor_health: &HashMap<&'static str, ActorStatus>,
    current_model: Option<&str>,
    model_popup: Option<&model_selector::ModelSelectorPopup>,
) {
    // ── Vertical layout: header / body / input ────────────────────────────────
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header bar
            Constraint::Min(1),    // Body area
            Constraint::Length(5), // Input area
        ])
        .split(frame.area());

    // ── Body: conversation (60%) + sidebar (40%) ──────────────────────────────
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main_chunks[1]);

    // Header
    let elapsed = stats.elapsed_formatted();
    let header_line = Line::from(vec![
        Span::styled(
            " \u{25c6} SENA",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  \u{2014}  local-first ambient AI  \u{2014}  daemon-connected",
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("    "),
        Span::styled("\u{25b8} ", Style::default().fg(Color::DarkGray)),
        Span::styled(elapsed, Style::default().fg(Color::White)),
    ]);
    let header = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
    );
    frame.render_widget(header, main_chunks[0]);

    // Conversation
    let mut lines = Vec::new();
    for msg in messages {
        match msg.role {
            MessageRole::User => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "\u{25b8} ",
                        Style::default()
                            .fg(Color::LightMagenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(&msg.text, Style::default().fg(Color::White)),
                ]));
                lines.push(Line::from("")); // spacing
            }
            MessageRole::Sena => {
                for line in msg.text.lines() {
                    lines.push(Line::from(Span::styled(
                        line,
                        Style::default().fg(Color::Green),
                    )));
                }
                lines.push(Line::from("")); // spacing
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
                        "\u{26a0} ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(&msg.text, Style::default().fg(Color::Yellow)),
                ]));
            }
        }
    }

    let total_lines = lines.len();
    let inner_height = body_chunks[0].height.saturating_sub(2) as usize; // subtract borders
    let scroll = if scroll_offset == 0 {
        total_lines.saturating_sub(inner_height)
    } else {
        total_lines.saturating_sub(inner_height + scroll_offset)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            " Conversation ",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));
    let paragraph = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, body_chunks[0]);

    // ── Sidebar ───────────────────────────────────────────────────────────────────
    render_ipc_sidebar(
        frame,
        body_chunks[1],
        loop_states,
        actor_health,
        current_model,
        stats,
    );

    // Input area
    let prompt_line = Line::from(vec![
        Span::styled(
            " sena ",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{203a} ", Style::default().fg(Color::DarkGray)),
        Span::raw(&editor.input),
        Span::styled("\u{258c}", Style::default().fg(Color::LightMagenta)),
    ]);

    let status_line = if let Some(first_press) = ctrl_c_first_press {
        if first_press.elapsed() < Duration::from_secs(3) {
            Line::from(Span::styled(
                " Press Ctrl+C again to exit",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(Span::styled(
                " Ready (IPC)",
                Style::default().fg(Color::DarkGray),
            ))
        }
    } else {
        Line::from(Span::styled(
            " Ready (IPC)",
            Style::default().fg(Color::DarkGray),
        ))
    };

    let hints_line = Line::from(Span::styled(
        " Tab:\u{2508}complete   \u{2191}\u{2193}:\u{2508}history   /help:\u{2508}commands",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            " Input ",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
    let input_widget =
        Paragraph::new(vec![prompt_line, status_line, hints_line]).block(input_block);
    frame.render_widget(input_widget, main_chunks[2]);

    // Slash dropdown overlay
    if let Some(dd) = slash_dropdown {
        if !dd.is_empty() && model_popup.is_none() {
            render_slash_dropdown_overlay(frame, dd, main_chunks[2]);
        }
    }

    // Model selector popup overlay
    if let Some(popup) = model_popup {
        model_selector::render_popup(popup, frame);
    }
}

/// Render the status sidebar for IPC mode — same layout as Shell::render_sidebar().
fn render_ipc_sidebar(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    loop_states: &HashMap<String, bool>,
    actor_health: &HashMap<&'static str, ActorStatus>,
    current_model: Option<&str>,
    stats: &SessionStats,
) {
    let mut lines: Vec<Line> = Vec::new();

    // ── Model ─────────────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        " Model",
        Style::default()
            .fg(Color::LightMagenta)
            .add_modifier(Modifier::BOLD),
    )));
    let model = current_model.unwrap_or("(selecting...)");
    let inner_w = area.width.saturating_sub(4) as usize;
    let display_model = if model.len() > inner_w {
        &model[..inner_w]
    } else {
        model
    };
    lines.push(Line::from(Span::styled(
        format!("  {}", display_model),
        Style::default().fg(Color::White),
    )));
    lines.push(Line::from(""));

    // ── Session stats ─────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        " Session",
        Style::default()
            .fg(Color::LightMagenta)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            stats.messages_sent.to_string(),
            Style::default().fg(Color::White),
        ),
        Span::styled(" messages  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            stats.tokens_received.to_string(),
            Style::default().fg(Color::White),
        ),
        Span::styled(" tokens", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(""));

    // ── Actors ────────────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        " Actors",
        Style::default()
            .fg(Color::LightMagenta)
            .add_modifier(Modifier::BOLD),
    )));
    let actor_names = ["Platform", "Inference", "CTP", "Memory", "Soul"];
    for name in &actor_names {
        let status = actor_health.get(name).unwrap_or(&ActorStatus::Starting);
        let (dot, color, label) = match status {
            ActorStatus::Ready => ("\u{25cf}", Color::Green, "Ready"),
            ActorStatus::Starting => ("\u{25cb}", Color::Yellow, "Starting"),
            ActorStatus::Failed(_) => ("\u{00d7}", Color::Red, "Failed"),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", dot), Style::default().fg(color)),
            Span::styled(format!("{:<10}", name), Style::default().fg(Color::White)),
            Span::styled(label, Style::default().fg(color)),
        ]));
    }
    lines.push(Line::from(""));

    // ── Loops ─────────────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        " Loops",
        Style::default()
            .fg(Color::LightMagenta)
            .add_modifier(Modifier::BOLD),
    )));
    let loop_order = [
        ("ctp", "CTP"),
        ("memory_consolidation", "Memory Consolidation"),
        ("platform_polling", "Platform Polling"),
        ("screen_capture", "Screen Capture"),
        ("speech", "Speech"),
    ];
    for (loop_name, display_name) in &loop_order {
        let enabled = loop_states.get(*loop_name).copied().unwrap_or(true);
        let color = if enabled { Color::Green } else { Color::Red };
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", "\u{25cf}"), Style::default().fg(color)),
            Span::styled(*display_name, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            " Status ",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));

    let paragraph = Paragraph::new(Text::from(lines)).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the slash command autocomplete dropdown for IPC mode.
fn render_slash_dropdown_overlay(
    frame: &mut ratatui::Frame,
    dd: &SlashDropdown,
    input_area: ratatui::layout::Rect,
) {
    use ratatui::widgets::Clear;

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
                        .fg(Color::LightMagenta)
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
                .border_style(Style::default().fg(Color::LightMagenta))
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

/// Boot the runtime and run the application. This is the main entry point
/// for both CLI and headless (tray) modes.
/// Open a CLI TUI session with an already-booted runtime.
///
/// Handles onboarding if needed, then runs the TUI. After the user exits,
/// triggers graceful shutdown of all actors.
///
/// NOTE: In Phase 6+, this function is not actively used by `sena cli` (which uses IPC mode instead).
/// It is kept for future testing or fallback scenarios.
#[allow(dead_code)]
pub async fn run_with_runtime(
    runtime: runtime::Runtime,
    needs_onboarding_flag: bool,
) -> anyhow::Result<()> {
    crate::display::banner();
    crate::display::success("Sena is ready.");

    let runtime_arc = Arc::new(runtime);
    let mut needs_onboarding = needs_onboarding_flag;

    run_onboarding_if_needed(&runtime_arc, &mut needs_onboarding).await?;

    let exit_reason = run(Arc::clone(&runtime_arc)).await;

    // Recover the Runtime from the Arc for shutdown.
    drop(exit_reason);
    let runtime = Arc::try_unwrap(runtime_arc)
        .map_err(|_| anyhow::anyhow!("runtime still referenced at CLI exit"))?;
    let timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    runtime::shutdown(runtime, timeout).await?;
    crate::display::success("Sena stopped cleanly.");
    Ok(())
}

async fn run_onboarding_if_needed(
    runtime: &Arc<Runtime>,
    needs_onboarding: &mut bool,
) -> anyhow::Result<()> {
    if !*needs_onboarding {
        return Ok(());
    }

    let models_available = runtime::ollama_models_dir()
        .ok()
        .and_then(|d| runtime::discover_models(&d).ok())
        .map(|r| !r.is_empty())
        .unwrap_or(false);

    let result = crate::onboarding::run_wizard(&runtime.bus, models_available).await?;

    let user_name = result.user_name.clone();
    let mut updated_config = runtime.config.clone();
    updated_config.file_watch_paths = result.file_watch_paths;
    updated_config.clipboard_observation_enabled = result.clipboard_observation_enabled;
    runtime::save_config(&updated_config).await?;
    crate::display::success(&format!("Onboarding saved for {}.", user_name));
    if let Ok(path) = runtime::config::config_path() {
        crate::display::info(&format!("Config file: {}", path.display()));
    }
    *needs_onboarding = false;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_exit_reason_close_and_shutdown_are_distinct() {
        assert_ne!(ShellExitReason::Close, ShellExitReason::Shutdown);
    }

    #[test]
    fn quit_alias_closes_cli_session() {
        assert_eq!(exit_command_result("/quit"), Some(DispatchResult::Close));
        assert_eq!(exit_command_result("/close"), Some(DispatchResult::Close));
    }

    #[test]
    fn shutdown_command_requests_full_app_shutdown() {
        assert_eq!(
            exit_command_result("/shutdown"),
            Some(DispatchResult::Shutdown)
        );
    }

    #[test]
    fn is_vision_capable_model_detects_known_patterns() {
        assert!(is_vision_capable_model("llava-1.6"));
        assert!(is_vision_capable_model("MiniCPM-V-2"));
        assert!(is_vision_capable_model("phi3-v-mini"));
        assert!(!is_vision_capable_model("llama3.2:3b"));
    }

    #[test]
    fn slash_commands_include_voice_and_screenshot() {
        assert!(SLASH_COMMANDS.iter().any(|cmd| cmd.command == "/voice"));
        assert!(SLASH_COMMANDS
            .iter()
            .any(|cmd| cmd.command == "/screenshot"));

        let voice_dropdown = SlashDropdown::from_prefix("/vo");
        assert_eq!(voice_dropdown.selected_command(), Some("/voice"));

        let screenshot_dropdown = SlashDropdown::from_prefix("/scre");
        assert_eq!(screenshot_dropdown.selected_command(), Some("/screenshot"));
    }

    #[test]
    fn format_memory_tui_fresh_state_shows_safe_message() {
        let output = format_memory_tui(&MemoryResponse {
            soul_summary: bus::events::transparency::SoulSummaryForTransparency {
                user_name: None,
                inference_cycle_count: 0,
                work_patterns: vec![],
                tool_preferences: vec![],
                interest_clusters: vec![],
            },
            memory_chunks: vec![],
        });

        assert!(output.contains("Soul Summary"));
        assert!(output.contains("No user memory is available yet"));
    }

    #[test]
    fn format_explanation_tui_lists_working_memory_when_present() {
        let output = format_explanation_tui(&InferenceExplanationResponse {
            request_context: "Explain the last answer".to_string(),
            response_text: "Here is why the answer was produced.".to_string(),
            working_memory_context: vec![bus::events::memory::MemoryChunk {
                text: "recent rust debugging context".to_string(),
                score: 0.91,
                timestamp: SystemTime::now(),
            }],
            rounds_completed: 1,
        });

        assert!(output.contains("Working Memory"));
        assert!(output.contains("recent rust debugging context"));
        assert!(!output.contains("none used"));
    }
}
