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
use bus::{DownloadEvent, Event, IpcPayload, TransparencyEvent as BusTransparencyEvent};
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

use crate::tui_state::{ActorStatus, Message, MessageRole};
use crate::{display, model_selector};
use runtime::boot::Runtime;
use std::path::PathBuf;

// Local-mode shell implementation retained for potential future offline/testing use.
// Phase 6+ uses IPC-only mode (run_with_ipc). These types support the local runtime
// path but are currently unreachable after removal of run_with_runtime().
#[allow(dead_code)]
const TRANSPARENCY_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

// ── Slash-command autocomplete ────────────────────────────────────────────────

/// Maximum input length in characters (Unit 20)
const MAX_INPUT_LENGTH: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandCategory {
    Chat,
    Transparency,
    Audio,
    System,
}

impl CommandCategory {
    fn label(&self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Transparency => "Transparency",
            Self::Audio => "Audio",
            Self::System => "System",
        }
    }
}

struct SlashCommand {
    command: &'static str,
    description: &'static str,
    category: CommandCategory,
}

const SLASH_COMMANDS: &[SlashCommand] = &[
    // Chat category
    SlashCommand {
        command: "/help",
        description: "Show all commands",
        category: CommandCategory::Chat,
    },
    SlashCommand {
        command: "/copy",
        description: "Copy the last response to clipboard",
        category: CommandCategory::Chat,
    },
    SlashCommand {
        command: "/models",
        description: "Select which model to use",
        category: CommandCategory::Chat,
    },
    SlashCommand {
        command: "/config",
        description: "Show settings (/config set <key> <value> or /config reload)",
        category: CommandCategory::Chat,
    },
    SlashCommand {
        command: "/reload",
        description: "Reload config from disk",
        category: CommandCategory::System,
    },
    // Transparency category
    SlashCommand {
        command: "/observation",
        description: "What are you observing right now?",
        category: CommandCategory::Transparency,
    },
    SlashCommand {
        command: "/memory",
        description: "What do you remember about me?",
        category: CommandCategory::Transparency,
    },
    SlashCommand {
        command: "/explanation",
        description: "Why did you say that?",
        category: CommandCategory::Transparency,
    },
    SlashCommand {
        command: "/actors",
        description: "Show actor health status",
        category: CommandCategory::Transparency,
    },
    SlashCommand {
        command: "/verbose",
        description: "Toggle verbose logging",
        category: CommandCategory::Transparency,
    },
    // Audio category
    SlashCommand {
        command: "/voice",
        description: "Toggle voice input in this CLI session",
        category: CommandCategory::Audio,
    },
    SlashCommand {
        command: "/speech",
        description: "View speech configuration and status",
        category: CommandCategory::Audio,
    },
    SlashCommand {
        command: "/listen",
        description: "Start/stop continuous live transcription",
        category: CommandCategory::Audio,
    },
    SlashCommand {
        command: "/microphone",
        description: "List microphones or select one (/microphone select <index>)",
        category: CommandCategory::Audio,
    },
    SlashCommand {
        command: "/stt-backend",
        description: "Switch STT backend (/stt-backend <whisper|sherpa|parakeet>)",
        category: CommandCategory::Audio,
    },
    // System category
    SlashCommand {
        command: "/screenshot",
        description: "Show screenshot capture + vision model status",
        category: CommandCategory::System,
    },
    SlashCommand {
        command: "/loops",
        description: "List/toggle background loops",
        category: CommandCategory::System,
    },
    SlashCommand {
        command: "/status",
        description: "Show system status overview",
        category: CommandCategory::System,
    },
    SlashCommand {
        command: "/help",
        description: "Show all commands",
        category: CommandCategory::System,
    },
    SlashCommand {
        command: "/close",
        description: "Close CLI (keep tray/runtime alive)",
        category: CommandCategory::System,
    },
    SlashCommand {
        command: "/shutdown",
        description: "Shut down Sena completely",
        category: CommandCategory::System,
    },
];

struct SlashDropdown {
    filtered: Vec<usize>,
    selected: usize,
    /// True if the current prefix matches no commands (Unit 20)
    no_matches: bool,
}

#[allow(dead_code)]
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
        let no_matches = filtered.is_empty() && !prefix.is_empty() && prefix != "/";
        Self {
            filtered,
            selected: 0,
            no_matches,
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
        self.no_matches = self.filtered.is_empty() && !prefix.is_empty() && prefix != "/";
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
#[allow(dead_code)]
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
#[allow(dead_code)]
struct Shell {
    /// Event bus for actor communication.
    bus: Arc<bus::EventBus>,
    /// Shared TUI state (messages, editor, stats, model, loops, actors, etc.)
    state: crate::tui_state::ShellState<SlashDropdown>,
    /// Request ID of the pending inference (if any).
    pending_inference_id: Option<u64>,
    /// Verbose mode: show all actor events.
    verbose: bool,
    /// Actors health popup visibility flag.
    actors_popup_visible: bool,
    /// True while waiting for a transparency query response on the bus.
    transparency_loading: bool,
    /// The currently pending transparency query, if any.
    pending_transparency: Option<PendingTransparencyQuery>,
    /// Runtime reference for config access.
    runtime: Arc<Runtime>,
    /// Shell-local voice UX state (does not persist config).
    voice_enabled: bool,
    /// Last emitted download-progress bucket (0..=10) keyed by request ID.
    download_progress: HashMap<u64, u64>,
}

impl Shell {
    /// Create a new Shell instance.
    fn new(runtime: Arc<Runtime>) -> Self {
        let voice_enabled = runtime.config.speech_enabled;

        // Initialize shared state with welcome messages.
        let mut state = crate::tui_state::ShellState::new();
        state.add_welcome_message("Welcome to Sena — local-first ambient AI");
        state.add_welcome_message("Type /help for commands, or chat freely.");

        // Seed current_model from runtime config.
        state.current_model = runtime.config.preferred_model.clone();

        Self {
            bus: runtime.bus.clone(),
            state,
            pending_inference_id: None,
            verbose: false,
            actors_popup_visible: false,
            transparency_loading: false,
            pending_transparency: None,
            runtime,
            voice_enabled,
            download_progress: HashMap::new(),
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
        if let Some(popup) = &self.state.model_popup {
            model_selector::render_popup(popup, frame);
        }
        if self.state.model_popup.is_none() && !self.actors_popup_visible {
            self.render_slash_dropdown(frame, main_chunks[2]);
        }
        if self.actors_popup_visible {
            self.render_actors_popup(frame);
        }
    }

    /// Render the compact header bar.
    fn render_header(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let elapsed = self.state.stats.elapsed_formatted();
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

        for msg in &self.state.messages {
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
        let scroll = if self.state.scroll_offset == 0 {
            total_lines.saturating_sub(inner_height)
        } else {
            total_lines.saturating_sub(inner_height + self.state.scroll_offset)
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
        // Calculate input length for char counter (Unit 20)
        let input_len = self.state.editor.input.chars().count();
        let threshold = (MAX_INPUT_LENGTH as f64 * 0.8) as usize;
        let show_char_counter = input_len > threshold;

        // Border color reflects current state
        let border_color = if self.state.waiting_for_inference || self.transparency_loading {
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
            Span::raw(&self.state.editor.input),
            Span::styled("\u{258c}", Style::default().fg(Color::LightMagenta)),
        ]);

        // ── Line 2: status ────────────────────────────────────────────────────
        let status_line = if let Some(first_press) = self.state.ctrl_c_first_press {
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
        } else if self.state.waiting_for_inference {
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

        // Title with optional char counter
        let title = if show_char_counter {
            format!(" Input [{}/{}] ", input_len, MAX_INPUT_LENGTH)
        } else {
            " Input ".to_string()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(
                title,
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
            .state
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
        let model = self
            .state
            .current_model
            .as_deref()
            .unwrap_or("(selecting...)");
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
                self.state.stats.messages_sent.to_string(),
                Style::default().fg(Color::White),
            ),
            Span::styled(" messages  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                self.state.stats.tokens_received.to_string(),
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
                .state
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
            let enabled = self
                .state
                .loop_states
                .get(*loop_name)
                .copied()
                .unwrap_or(true);
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
                .state
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

        let Some(ref dd) = self.state.slash_dropdown else {
            return;
        };

        // Unit 20: Show "no matching commands" when filter is empty
        if dd.no_matches {
            let popup_height = 3u16; // border + single line
            let popup_width = 40u16.min(frame.area().width.saturating_sub(4));
            let y = input_area.y.saturating_sub(popup_height);
            let popup_area = ratatui::layout::Rect {
                x: input_area.x + 2,
                y,
                width: popup_width,
                height: popup_height,
            };
            frame.render_widget(Clear, popup_area);
            let no_match_line = Line::from(Span::styled(
                "no matching commands",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ));
            let para = Paragraph::new(no_match_line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
            frame.render_widget(para, popup_area);
            return;
        }

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
                // Unit 19: Show category label next to command
                ListItem::new(Line::from(vec![
                    Span::styled(
                        cmd.command,
                        Style::default()
                            .fg(Color::LightMagenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("[{}]", cmd.category.label()),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
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
        self.state.messages.push(Message::new(role, text));
        // Auto-scroll to bottom when new message arrives (unless user scrolled up)
        if self.state.scroll_offset == 0 {
            // Already at bottom, stay there
        }
    }

    /// Update the last streaming interim message in-place or add a new one.
    ///
    /// Used for streaming transcription so interim `[…] text` updates replace each
    /// other rather than flooding the conversation log with duplicate lines.
    fn update_or_add_streaming(&mut self, text: String) {
        let prefix = "[\u{2026}] ";
        let full = format!("{}{}", prefix, text);
        // Replace the last message if it is already a streaming interim line.
        if let Some(last) = self.state.messages.last_mut() {
            if last.role == MessageRole::System && last.text.starts_with(prefix) {
                last.text = full;
                return;
            }
        }
        self.state.messages.push(Message::new(MessageRole::System, full));
    }

    /// Handle bus events and update internal state.
    async fn handle_bus_event(&mut self, event: Event) {
        match event {
            Event::System(bus::events::SystemEvent::ActorReady { actor_name }) => {
                if let Some(status) = self.state.actor_health.get_mut(actor_name) {
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
                self.state.waiting_for_inference = false;
                if text.trim().is_empty() {
                    self.add_message(
                        MessageRole::Warning,
                        "Model returned empty response".to_string(),
                    );
                } else {
                    self.add_message(MessageRole::Sena, text);
                    self.state.stats.tokens_received += token_count;
                }
            }
            Event::Inference(InferenceEvent::InferenceFailed { request_id, reason })
                if self.pending_inference_id == Some(request_id) =>
            {
                self.pending_inference_id = None;
                self.state.waiting_for_inference = false;
                // Unit 21: Parse and format inference errors to be more actionable
                let error_message =
                    format_inference_error(&reason, self.state.current_model.as_deref());
                self.add_message(MessageRole::Warning, error_message);
            }
            Event::Inference(InferenceEvent::ModelLoaded { name, backend }) => {
                if self.verbose || self.state.waiting_for_inference {
                    self.add_message(
                        MessageRole::System,
                        format!("Model loaded: {} ({})", name, backend),
                    );
                }
                self.state.current_model = Some(name);
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
                if let Some(status) = self.state.actor_health.get_mut(info.actor_name) {
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
                    let model = self.state.current_model.as_deref().unwrap_or("(unknown)");
                    self.add_message(
                        MessageRole::System,
                        format!(
                            "Status: ready • model={} • messages={} • tokens={}",
                            model, self.state.stats.messages_sent, self.state.stats.tokens_received
                        ),
                    );
                }
                bus::events::TrayMenuItem::ShowLastThought => {
                    if let Some(last_text) = self
                        .state
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
                        Some("[you]: "),
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
            Event::Speech(SpeechEvent::LowConfidenceTranscription { confidence, .. }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!(
                        "Speech detected but confidence too low ({:.0}%). Please try again.",
                        confidence * 100.0
                    ),
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
                if !self.state.listen_mode_active {
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
            }) if session_id == self.state.listen_session_id => {
                if confidence < 0.6 {
                    self.add_message(
                        MessageRole::Warning,
                        format!("[listen ~{:.0}%] {}", confidence * 100.0, text),
                    );
                } else if is_final {
                    // Committed utterance: replace any in-flight interim line with the final.
                    if let Some(last) = self.state.messages.last() {
                        if last.role == MessageRole::System && last.text.starts_with("[\u{2026}] ") {
                            self.state.messages.pop();
                        }
                    }
                    self.add_message(MessageRole::Sena, text);
                } else {
                    // Streaming interim update — replace in-place, do not flood the log.
                    self.update_or_add_streaming(text);
                }
            }
            Event::System(bus::events::SystemEvent::LoopStatusChanged { loop_name, enabled }) => {
                self.state.loop_states.insert(loop_name.clone(), enabled);
                if self.verbose {
                    let status = if enabled { "enabled" } else { "disabled" };
                    self.add_message(
                        MessageRole::System,
                        format!("[verbose] Loop {}: {}", loop_name, status),
                    );
                }
            }
            Event::Speech(SpeechEvent::ListenModeStopped { session_id })
                if session_id == self.state.listen_session_id =>
            {
                self.state.listen_mode_active = false;
                self.add_message(
                    MessageRole::System,
                    "\u{1f3a4} Listen mode stopped.".to_string(),
                );
            }
            Event::Speech(SpeechEvent::SttBackendSwitchCompleted { backend }) => {
                self.add_message(
                    MessageRole::System,
                    format!("✓ STT backend switched to: {}", backend),
                );
            }
            Event::Speech(SpeechEvent::SttBackendSwitchFailed { backend, reason }) => {
                self.add_message(
                    MessageRole::Warning,
                    format!("Failed to switch to {}: {}", backend, reason),
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
        if line.starts_with('/') || self.state.pending_model_dir_input {
            let mut command_state = CommandRuntimeState {
                verbose: self.verbose,
                voice_enabled: self.voice_enabled,
                transparency_loading: self.transparency_loading,
                pending_transparency: self.pending_transparency.take(),
            };
            let mut deps = CommandDeps {
                runtime: Some(self.runtime.as_ref()),
                state: &mut self.state,
                command_state: &mut command_state,
            };
            let mut target = DispatchTarget::LocalBus(&self.bus);
            let mut transport = LocalCommandDispatchTransport;
            match dispatch_command(&line, &mut transport, &mut target, &mut deps).await {
                Ok(result) => {
                    self.verbose = command_state.verbose;
                    self.voice_enabled = command_state.voice_enabled;
                    self.transparency_loading = command_state.transparency_loading;
                    self.pending_transparency = command_state.pending_transparency;
                    return result;
                }
                Err(e) => {
                    self.add_message(MessageRole::Warning, format!("Command failed: {}", e));
                    self.verbose = command_state.verbose;
                    self.voice_enabled = command_state.voice_enabled;
                    self.transparency_loading = command_state.transparency_loading;
                    self.pending_transparency = command_state.pending_transparency;
                    return DispatchResult::Continue;
                }
            }
        }

        // Free text -> inference chat
        self.send_chat(line).await;
        DispatchResult::Continue
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
        self.state.waiting_for_inference = true;
        self.pending_inference_id = Some(request_id);
        self.state.stats.messages_sent += 1;

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
            self.state.waiting_for_inference = false;
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

    /// Copy the most recent Sena response to the system clipboard.
    fn copy_last_response(&mut self) {
        copy_last_response_shared(&mut self.state);
    }
}

#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum DispatchResult {
    Continue,
    Close,
    Shutdown,
}

enum DispatchTarget<'a> {
    LocalBus(&'a Arc<bus::EventBus>),
    Noop,
}

impl<'a> DispatchTarget<'a> {
    async fn send_event(&mut self, event: Event) -> Result<()> {
        match self {
            Self::LocalBus(bus) => {
                if matches!(
                    event,
                    Event::Inference(InferenceEvent::InferenceRequested { .. })
                ) {
                    bus.send_directed("inference", event)
                        .await
                        .map_err(|e| anyhow::anyhow!("directed send failed: {}", e))
                } else {
                    bus.broadcast(event)
                        .await
                        .map_err(|e| anyhow::anyhow!("bus broadcast failed: {}", e))
                }
            }
            Self::Noop => Err(anyhow::anyhow!(
                "dispatch target does not support send_event"
            )),
        }
    }

    #[allow(dead_code)]
    async fn send_ipc(&mut self, payload: IpcPayload) -> Result<()> {
        match self {
            Self::LocalBus(_) => Ok(()),
            Self::Noop => {
                let _ = payload;
                Ok(())
            }
        }
    }
}

enum DisplayMessage {
    BusEvent(Box<Event>),
    IpcPayload(IpcPayload),
}

trait MessageTransport: Send {
    async fn recv(&mut self) -> Option<DisplayMessage>;
    async fn send(&mut self, target: &mut DispatchTarget<'_>, event: Event) -> Result<()>;
}

pub struct LocalBusTransport {
    rx: tokio::sync::broadcast::Receiver<Event>,
}

impl LocalBusTransport {
    fn new(rx: tokio::sync::broadcast::Receiver<Event>) -> Self {
        Self { rx }
    }
}

impl MessageTransport for LocalBusTransport {
    async fn recv(&mut self) -> Option<DisplayMessage> {
        match self.rx.recv().await {
            Ok(event) => Some(DisplayMessage::BusEvent(Box::new(event))),
            Err(_) => None,
        }
    }

    async fn send(&mut self, target: &mut DispatchTarget<'_>, event: Event) -> Result<()> {
        target.send_event(event).await
    }
}

pub struct IpcTransport {
    client: crate::ipc_client::IpcClient,
}

impl IpcTransport {
    fn new(client: crate::ipc_client::IpcClient) -> Self {
        Self { client }
    }
}

impl MessageTransport for IpcTransport {
    async fn recv(&mut self) -> Option<DisplayMessage> {
        self.client
            .recv()
            .await
            .map(|msg| DisplayMessage::IpcPayload(msg.payload))
    }

    async fn send(&mut self, _target: &mut DispatchTarget<'_>, event: Event) -> Result<()> {
        let payload = event_to_ipc_payload(&event)?;
        self.client
            .send(payload)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("IPC send failed: {}", e))
    }
}

struct LocalCommandDispatchTransport;

impl MessageTransport for LocalCommandDispatchTransport {
    async fn recv(&mut self) -> Option<DisplayMessage> {
        None
    }

    async fn send(&mut self, target: &mut DispatchTarget<'_>, event: Event) -> Result<()> {
        target.send_event(event).await
    }
}

#[derive(Default)]
struct CommandRuntimeState {
    verbose: bool,
    voice_enabled: bool,
    transparency_loading: bool,
    pending_transparency: Option<PendingTransparencyQuery>,
}

struct CommandDeps<'a> {
    runtime: Option<&'a Runtime>,
    state: &'a mut crate::tui_state::ShellState<SlashDropdown>,
    command_state: &'a mut CommandRuntimeState,
}

async fn dispatch_command<T: MessageTransport + ?Sized>(
    input: &str,
    transport: &mut T,
    target: &mut DispatchTarget<'_>,
    deps: &mut CommandDeps<'_>,
) -> Result<DispatchResult> {
    let line = input.trim();
    if line.is_empty() {
        return Ok(DispatchResult::Continue);
    }

    if deps.state.pending_model_dir_input {
        deps.state.pending_model_dir_input = false;
        handle_model_dir_input_shared(line, deps.state)?;
        return Ok(DispatchResult::Continue);
    }

    if !line.starts_with('/') {
        return Ok(DispatchResult::Continue);
    }

    let lower = line.to_lowercase();
    let cmd = lower.split_whitespace().next().unwrap_or("");
    match cmd {
        "/observation" | "/obs" => {
            add_message(
                deps.state,
                MessageRole::System,
                "Querying current observation...",
            );
            deps.command_state.transparency_loading = true;
            deps.command_state.pending_transparency = Some(PendingTransparencyQuery {
                query: TransparencyQuery::CurrentObservation,
                started_at: Instant::now(),
            });
            transport
                .send(
                    target,
                    Event::Transparency(BusTransparencyEvent::QueryRequested(
                        TransparencyQuery::CurrentObservation,
                    )),
                )
                .await?;
            Ok(DispatchResult::Continue)
        }
        "/memory" | "/mem" => {
            add_message(deps.state, MessageRole::System, "Querying memory...");
            deps.command_state.transparency_loading = true;
            deps.command_state.pending_transparency = Some(PendingTransparencyQuery {
                query: TransparencyQuery::UserMemory,
                started_at: Instant::now(),
            });
            transport
                .send(
                    target,
                    Event::Transparency(BusTransparencyEvent::QueryRequested(
                        TransparencyQuery::UserMemory,
                    )),
                )
                .await?;
            Ok(DispatchResult::Continue)
        }
        "/explanation" | "/why" => {
            add_message(
                deps.state,
                MessageRole::System,
                "Querying last inference...",
            );
            deps.command_state.transparency_loading = true;
            deps.command_state.pending_transparency = Some(PendingTransparencyQuery {
                query: TransparencyQuery::InferenceExplanation,
                started_at: Instant::now(),
            });
            transport
                .send(
                    target,
                    Event::Transparency(BusTransparencyEvent::QueryRequested(
                        TransparencyQuery::InferenceExplanation,
                    )),
                )
                .await?;
            Ok(DispatchResult::Continue)
        }
        "/models" => {
            let models_dir = current_models_dir(deps.runtime).await?;
            let models = model_selector::discover_models_at(models_dir.as_path())?;
            deps.state.model_popup = Some(model_selector::ModelSelectorPopup::new(models));
            Ok(DispatchResult::Continue)
        }
        "/verbose" => {
            deps.command_state.verbose = !deps.command_state.verbose;
            let status = if deps.command_state.verbose {
                "ON"
            } else {
                "OFF"
            };
            add_message(
                deps.state,
                MessageRole::System,
                &format!("Verbose logging: {}", status),
            );
            Ok(DispatchResult::Continue)
        }
        "/voice" => {
            let speech_enabled = load_speech_enabled(deps.runtime).await?;
            if !speech_enabled {
                add_message(
                    deps.state,
                    MessageRole::Warning,
                    "Voice is unavailable because speech is disabled in config.",
                );
                return Ok(DispatchResult::Continue);
            }

            deps.command_state.voice_enabled = !deps.command_state.voice_enabled;
            let state = if deps.command_state.voice_enabled {
                "ON"
            } else {
                "OFF"
            };
            // Broadcast speech loop control to the daemon.
            transport
                .send(
                    target,
                    Event::System(bus::events::SystemEvent::LoopControlRequested {
                        loop_name: "speech".to_string(),
                        enabled: deps.command_state.voice_enabled,
                    }),
                )
                .await
                .ok();
            add_message(
                deps.state,
                MessageRole::System,
                &format!("VOICE: {}", state),
            );
            add_message(
                deps.state,
                MessageRole::System,
                "Voice input toggled for this CLI session; persistent runtime speech settings remain in config.",
            );
            Ok(DispatchResult::Continue)
        }
        "/speech" => {
            show_speech_config_shared(deps.runtime, deps.state, deps.command_state.voice_enabled)
                .await;
            Ok(DispatchResult::Continue)
        }
        "/listen" => {
            if deps.state.listen_mode_active {
                let session_id = deps.state.listen_session_id;
                deps.state.listen_mode_active = false;
                transport
                    .send(
                        target,
                        Event::Speech(SpeechEvent::ListenModeStopRequested { session_id }),
                    )
                    .await?;
                add_message(
                    deps.state,
                    MessageRole::System,
                    "🎤 Stopping listen mode...",
                );
            } else {
                let speech_enabled = load_speech_enabled(deps.runtime).await?;
                if !speech_enabled {
                    add_message(
                        deps.state,
                        MessageRole::Warning,
                        "Speech must be enabled for /listen. Use /config set speech_enabled true",
                    );
                    return Ok(DispatchResult::Continue);
                }

                deps.state.listen_session_id = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(1);
                deps.state.listen_mode_active = true;
                let session_id = deps.state.listen_session_id;
                transport
                    .send(
                        target,
                        Event::Speech(SpeechEvent::ListenModeRequested { session_id }),
                    )
                    .await?;
                add_message(
                    deps.state,
                    MessageRole::System,
                    "🎤 Listen mode started — type /listen again to stop.",
                );
            }
            Ok(DispatchResult::Continue)
        }
        "/microphone" => {
            handle_microphone_command(line, transport, target, deps.state).await?;
            Ok(DispatchResult::Continue)
        }
        "/stt-backend" => {
            handle_stt_backend_command(line, transport, target, deps.state).await?;
            Ok(DispatchResult::Continue)
        }
        "/screenshot" => {
            show_screenshot_status_shared(deps.runtime, deps.state).await;
            Ok(DispatchResult::Continue)
        }
        "/config" => {
            handle_config_command(line, transport, target, deps.state).await?;
            Ok(DispatchResult::Continue)
        }
        "/reload" => {
            transport
                .send(
                    target,
                    Event::System(bus::events::SystemEvent::ConfigReloadRequested),
                )
                .await?;
            add_message(
                deps.state,
                MessageRole::System,
                "Reloading config from disk...",
            );
            Ok(DispatchResult::Continue)
        }
        "/loops" => {
            handle_loops_command(line, transport, target, deps.state).await?;
            Ok(DispatchResult::Continue)
        }
        "/status" => {
            show_status_shared(deps.runtime, deps.state).await;
            Ok(DispatchResult::Continue)
        }
        "/help" | "/h" => {
            show_help_shared(deps.state);
            Ok(DispatchResult::Continue)
        }
        "/actors" => {
            show_actors_shared(deps.state);
            Ok(DispatchResult::Continue)
        }
        "/copy" => {
            copy_last_response_shared(deps.state);
            Ok(DispatchResult::Continue)
        }
        "/close" | "/quit" | "/exit" | "/q" => Ok(DispatchResult::Close),
        "/shutdown" => {
            transport
                .send(
                    target,
                    Event::System(bus::events::SystemEvent::ShutdownSignal),
                )
                .await?;
            Ok(DispatchResult::Shutdown)
        }
        _ => {
            add_message(
                deps.state,
                MessageRole::Warning,
                &format!("Unknown command '{}'. Type /help for commands.", line),
            );
            Ok(DispatchResult::Continue)
        }
    }
}

fn add_message(
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
    role: MessageRole,
    text: &str,
) {
    state.messages.push(Message::new(role, text.to_string()));
}

fn event_to_ipc_payload(event: &Event) -> Result<IpcPayload> {
    match event {
        Event::Transparency(BusTransparencyEvent::QueryRequested(
            TransparencyQuery::CurrentObservation,
        )) => Ok(IpcPayload::SlashCommand {
            line: "/observation".to_string(),
        }),
        Event::Transparency(BusTransparencyEvent::QueryRequested(
            TransparencyQuery::UserMemory,
        )) => Ok(IpcPayload::SlashCommand {
            line: "/memory".to_string(),
        }),
        Event::Transparency(BusTransparencyEvent::QueryRequested(
            TransparencyQuery::InferenceExplanation,
        )) => Ok(IpcPayload::SlashCommand {
            line: "/explanation".to_string(),
        }),
        Event::Speech(SpeechEvent::ListenModeRequested { session_id }) => {
            Ok(IpcPayload::SlashCommand {
                line: format!("/listen start {}", session_id),
            })
        }
        Event::Speech(SpeechEvent::ListenModeStopRequested { session_id }) => {
            Ok(IpcPayload::SlashCommand {
                line: format!("/listen stop {}", session_id),
            })
        }
        Event::System(bus::events::SystemEvent::ConfigSetRequested { key, value }) => {
            Ok(IpcPayload::SlashCommand {
                line: format!("/config set {} {}", key, value),
            })
        }
        Event::System(bus::events::SystemEvent::ConfigReloadRequested) => {
            Ok(IpcPayload::SlashCommand {
                line: "/config reload".to_string(),
            })
        }
        Event::System(bus::events::SystemEvent::LoopControlRequested { loop_name, enabled }) => {
            Ok(IpcPayload::SlashCommand {
                line: format!(
                    "/loops {} {}",
                    loop_name,
                    if *enabled { "on" } else { "off" }
                ),
            })
        }
        Event::Speech(SpeechEvent::SttBackendSwitchRequested { backend }) => {
            Ok(IpcPayload::SlashCommand {
                line: format!("/stt-backend {}", backend),
            })
        }
        Event::System(bus::events::SystemEvent::ShutdownSignal) => {
            Ok(IpcPayload::ShutdownRequested)
        }
        _ => Err(anyhow::anyhow!("event cannot be sent over IPC target")),
    }
}

async fn current_models_dir(runtime: Option<&Runtime>) -> Result<PathBuf> {
    if let Some(runtime) = runtime {
        if let Some(path) = runtime.config.models_dir.clone() {
            return Ok(path);
        }
    } else if let Ok(config) = runtime::config::load_or_create_config().await {
        if let Some(path) = config.models_dir {
            return Ok(path);
        }
    }

    runtime::ollama_models_dir()
        .map_err(|e| anyhow::anyhow!("Could not resolve models directory: {}", e))
}

async fn load_speech_enabled(runtime: Option<&Runtime>) -> Result<bool> {
    if let Some(runtime) = runtime {
        return Ok(runtime.config.speech_enabled);
    }

    runtime::config::load_or_create_config()
        .await
        .map(|c| c.speech_enabled)
        .map_err(|e| anyhow::anyhow!("Could not read config: {}", e))
}

async fn show_speech_config_shared(
    runtime: Option<&Runtime>,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
    voice_enabled: bool,
) {
    let config = if let Some(runtime) = runtime {
        runtime.config.clone()
    } else {
        match runtime::config::load_or_create_config().await {
            Ok(c) => c,
            Err(e) => {
                add_message(
                    state,
                    MessageRole::Warning,
                    &format!("Failed to load config: {}", e),
                );
                return;
            }
        }
    };

    add_message(state, MessageRole::System, "━━  Speech Configuration");
    add_message(
        state,
        MessageRole::System,
        &format!("speech_enabled            {}", config.speech_enabled),
    );
    add_message(
        state,
        MessageRole::System,
        &format!(
            "speech_model_dir          {}",
            config
                .speech_model_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(default platform path)".to_string())
        ),
    );
    add_message(
        state,
        MessageRole::System,
        &format!(
            "voice_always_listening    {}",
            config.voice_always_listening
        ),
    );
    add_message(
        state,
        MessageRole::System,
        &format!("wakeword_enabled          {}", config.wakeword_enabled),
    );
    add_message(
        state,
        MessageRole::System,
        &format!("tts_rate                  {:.1}", config.tts_rate),
    );
    add_message(
        state,
        MessageRole::System,
        &format!(
            "proactive_speech_enabled  {}",
            config.proactive_speech_enabled
        ),
    );
    add_message(
        state,
        MessageRole::System,
        &format!(
            "voice_session_active      {}",
            if voice_enabled { "yes" } else { "no" }
        ),
    );
}

async fn show_screenshot_status_shared(
    runtime: Option<&Runtime>,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
) {
    let capture_enabled = if let Some(runtime) = runtime {
        runtime.config.screen_capture_enabled
    } else {
        runtime::config::load_or_create_config()
            .await
            .map(|c| c.screen_capture_enabled)
            .unwrap_or(false)
    };
    let platform_support = if cfg!(target_os = "windows") {
        "supported"
    } else {
        "not implemented"
    };
    let active_model = state.current_model.as_deref().unwrap_or("unknown");
    let vision_status = if is_vision_capable_model(active_model) {
        "yes"
    } else {
        "no"
    };
    add_message(
        state,
        MessageRole::System,
        &format!(
            "Screenshot capture: {} | Platform: {} | Active model: {} | Vision capable: {}",
            if capture_enabled {
                "enabled"
            } else {
                "disabled"
            },
            platform_support,
            active_model,
            vision_status
        ),
    );
    add_message(
        state,
        MessageRole::System,
        "Privacy: screenshots are in-memory only and not persisted. Availability depends on platform support.",
    );
}

async fn handle_microphone_command<T: MessageTransport + ?Sized>(
    line: &str,
    transport: &mut T,
    target: &mut DispatchTarget<'_>,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
) -> Result<()> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.get(1) == Some(&"select") {
        let idx_str = parts.get(2).copied().unwrap_or("");
        let idx: usize = idx_str
            .parse()
            .map_err(|_| anyhow::anyhow!("Usage: /microphone select <index>"))?;
        let devices = runtime::list_input_devices();
        if let Some((_, name)) = devices.into_iter().find(|(i, _)| *i == idx) {
            let value = if idx == 0 {
                String::new()
            } else {
                name.clone()
            };
            transport
                .send(
                    target,
                    Event::System(bus::events::SystemEvent::ConfigSetRequested {
                        key: "microphone_device".to_string(),
                        value,
                    }),
                )
                .await?;
            add_message(
                state,
                MessageRole::System,
                &format!(
                    "🎤 Microphone set to: {}{}",
                    name,
                    if idx == 0 { " (system default)" } else { "" }
                ),
            );
        } else {
            add_message(
                state,
                MessageRole::Warning,
                &format!(
                    "No device at index {}. Run /microphone to list devices.",
                    idx
                ),
            );
        }
    } else {
        let devices = runtime::list_input_devices();
        add_message(state, MessageRole::System, "━━  Available Microphones");
        if devices.is_empty() {
            add_message(state, MessageRole::Warning, "No input devices found.");
        } else {
            for (idx, name) in &devices {
                add_message(
                    state,
                    MessageRole::System,
                    &format!("  [{}]  {}", idx, name),
                );
            }
            add_message(
                state,
                MessageRole::System,
                "Use /microphone select <index> to switch. Index 0 = system default.",
            );
        }
    }
    Ok(())
}

async fn handle_config_command<T: MessageTransport + ?Sized>(
    line: &str,
    transport: &mut T,
    target: &mut DispatchTarget<'_>,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
) -> Result<()> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.get(1) == Some(&"set") {
        let key = parts.get(2).copied().unwrap_or("");
        let value = if parts.len() > 3 {
            parts[3..].join(" ")
        } else {
            String::new()
        };
        transport
            .send(
                target,
                Event::System(bus::events::SystemEvent::ConfigSetRequested {
                    key: key.to_string(),
                    value: value.clone(),
                }),
            )
            .await?;
        add_message(
            state,
            MessageRole::System,
            &format!("Setting {} = {}...", key, value),
        );
    } else if parts.get(1) == Some(&"reload") {
        transport
            .send(
                target,
                Event::System(bus::events::SystemEvent::ConfigReloadRequested),
            )
            .await?;
        add_message(state, MessageRole::System, "Reloading config from disk...");
    } else {
        let config_path = runtime::config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(unavailable)".to_string());
        let config = runtime::config::load_or_create_config().await?;

        add_message(state, MessageRole::System, "━━  Configuration");
        add_message(
            state,
            MessageRole::System,
            &format!("Config file: {}", config_path),
        );
        match toml::to_string_pretty(&config) {
            Ok(toml_str) => {
                for line in toml_str.lines() {
                    add_message(state, MessageRole::System, line);
                }
            }
            Err(e) => {
                add_message(
                    state,
                    MessageRole::Warning,
                    &format!("Could not serialize config: {}", e),
                );
            }
        }
    }
    Ok(())
}

async fn handle_loops_command<T: MessageTransport + ?Sized>(
    line: &str,
    transport: &mut T,
    target: &mut DispatchTarget<'_>,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
) -> Result<()> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        ["/loops"] => {
            add_message(state, MessageRole::System, "━━  Background Loops");
            let loop_order = [
                ("ctp", "CTP"),
                ("memory_consolidation", "Memory Consolidation"),
                ("platform_polling", "Platform Polling"),
                ("screen_capture", "Screen Capture"),
                ("speech", "Speech"),
            ];
            for (name, label) in &loop_order {
                let enabled = state.loop_states.get(*name).copied().unwrap_or(true);
                add_message(
                    state,
                    MessageRole::System,
                    &format!(
                        "  {} — {}",
                        label,
                        if enabled { "enabled" } else { "disabled" }
                    ),
                );
            }
        }
        ["/loops", name] => {
            let enabled = !state.loop_states.get(*name).copied().unwrap_or(true);
            transport
                .send(
                    target,
                    Event::System(bus::events::SystemEvent::LoopControlRequested {
                        loop_name: (*name).to_string(),
                        enabled,
                    }),
                )
                .await?;
            add_message(
                state,
                MessageRole::System,
                &format!(
                    "{} loop '{}'...",
                    if enabled { "Enabling" } else { "Disabling" },
                    name
                ),
            );
        }
        ["/loops", name, state_arg] => {
            let enabled = match state_arg.to_ascii_lowercase().as_str() {
                "on" | "enable" | "true" => true,
                "off" | "disable" | "false" => false,
                _ => {
                    add_message(state, MessageRole::Warning, "Invalid state. Use on or off.");
                    return Ok(());
                }
            };
            transport
                .send(
                    target,
                    Event::System(bus::events::SystemEvent::LoopControlRequested {
                        loop_name: (*name).to_string(),
                        enabled,
                    }),
                )
                .await?;
            add_message(
                state,
                MessageRole::System,
                &format!(
                    "{} loop '{}'...",
                    if enabled { "Enabling" } else { "Disabling" },
                    name
                ),
            );
        }
        _ => add_message(
            state,
            MessageRole::Warning,
            "Usage: /loops | /loops <name> | /loops <name> on|off",
        ),
    }
    Ok(())
}

async fn handle_stt_backend_command<T: MessageTransport + ?Sized>(
    line: &str,
    transport: &mut T,
    target: &mut DispatchTarget<'_>,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
) -> Result<()> {
    let parts: Vec<&str> = line.split_whitespace().collect();

    match parts.as_slice() {
        ["/stt-backend"] => {
            // Show current backend + valid options
            let config = runtime::config::load_or_create_config().await?;
            add_message(state, MessageRole::System, "━━  STT Backend");
            add_message(
                state,
                MessageRole::System,
                &format!("Current: {:?}", config.stt_backend),
            );
            add_message(
                state,
                MessageRole::System,
                "Available: whisper, sherpa, parakeet",
            );
            add_message(state, MessageRole::System, "Usage: /stt-backend <backend>");
        }
        ["/stt-backend", backend_str] => {
            // Prevent switch during active listen
            if state.listen_mode_active {
                add_message(
                    state,
                    MessageRole::Warning,
                    "Cannot switch STT backend during /listen. Stop listening first.",
                );
                return Ok(());
            }

            // Validate backend name
            let backend = match backend_str.to_lowercase().as_str() {
                "whisper" | "sherpa" | "parakeet" => backend_str.to_lowercase(),
                _ => {
                    add_message(
                        state,
                        MessageRole::Warning,
                        "Invalid backend. Use: whisper, sherpa, parakeet",
                    );
                    return Ok(());
                }
            };

            // Broadcast switch request
            transport
                .send(
                    target,
                    Event::Speech(SpeechEvent::SttBackendSwitchRequested {
                        backend: backend.clone(),
                    }),
                )
                .await?;

            add_message(
                state,
                MessageRole::System,
                &format!("Switching STT backend to {}...", backend),
            );
        }
        _ => {
            add_message(
                state,
                MessageRole::Warning,
                "Usage: /stt-backend | /stt-backend <whisper|sherpa|parakeet>",
            );
        }
    }
    Ok(())
}

fn show_help_shared(state: &mut crate::tui_state::ShellState<SlashDropdown>) {
    add_message(state, MessageRole::System, "━━  Commands");

    for category in [
        CommandCategory::Chat,
        CommandCategory::Transparency,
        CommandCategory::Audio,
        CommandCategory::System,
    ] {
        add_message(
            state,
            MessageRole::System,
            &format!("{}:", category.label()),
        );

        for cmd in SLASH_COMMANDS.iter().filter(|cmd| cmd.category == category) {
            add_message(
                state,
                MessageRole::System,
                &format!("  {:<20} {}", cmd.command, cmd.description),
            );
        }

        if category == CommandCategory::Transparency {
            add_message(
                state,
                MessageRole::System,
                "  /observation or /obs  Alias for /observation",
            );
            add_message(
                state,
                MessageRole::System,
                "  /memory or /mem       Alias for /memory",
            );
            add_message(
                state,
                MessageRole::System,
                "  /explanation or /why  Alias for /explanation",
            );
        }

        if category == CommandCategory::System {
            add_message(
                state,
                MessageRole::System,
                "  /close or /quit       Alias to close the CLI session",
            );
        }

        add_message(state, MessageRole::System, "");
    }
}

fn show_actors_shared(state: &mut crate::tui_state::ShellState<SlashDropdown>) {
    add_message(state, MessageRole::System, "━━  Actor Status");
    for name in ["Platform", "Inference", "CTP", "Memory", "Soul"] {
        let status = state
            .actor_health
            .get(name)
            .unwrap_or(&ActorStatus::Starting);
        let text = match status {
            ActorStatus::Ready => "Ready".to_string(),
            ActorStatus::Starting => "Starting".to_string(),
            ActorStatus::Failed(reason) => format!("Failed: {}", reason),
        };
        add_message(state, MessageRole::System, &format!("{}  {}", name, text));
    }
}

async fn show_status_shared(
    runtime: Option<&Runtime>,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
) {
    add_message(state, MessageRole::Sena, "== System Status ==");

    add_message(state, MessageRole::System, "Uptime");
    add_message(
        state,
        MessageRole::System,
        &format!("  {}", state.stats.elapsed_formatted()),
    );

    let model = state
        .current_model
        .clone()
        .unwrap_or_else(|| "(no model loaded)".to_string());
    add_message(state, MessageRole::System, "Active Model");
    add_message(state, MessageRole::System, &format!("  {}", model));

    add_message(state, MessageRole::System, "Actor Health");
    let mut actors: Vec<(String, ActorStatus)> = state
        .actor_health
        .iter()
        .map(|(name, status)| ((*name).to_string(), status.clone()))
        .collect();
    actors.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, status) in actors {
        let line = match status {
            ActorStatus::Ready => format!("  {}: Ready", name),
            ActorStatus::Starting => format!("  {}: Starting", name),
            ActorStatus::Failed(reason) => format!("  {}: Failed ({})", name, reason),
        };
        add_message(state, MessageRole::System, &line);
    }

    add_message(state, MessageRole::System, "Loop States");
    let mut loops: Vec<(String, bool)> = state
        .loop_states
        .iter()
        .map(|(name, enabled)| (name.clone(), *enabled))
        .collect();
    loops.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, enabled) in loops {
        add_message(
            state,
            MessageRole::System,
            &format!(
                "  {}: {}",
                name,
                if enabled { "enabled" } else { "disabled" }
            ),
        );
    }

    let speech_enabled = if let Some(runtime) = runtime {
        runtime.config.speech_enabled
    } else {
        runtime::config::load_or_create_config()
            .await
            .map(|c| c.speech_enabled)
            .unwrap_or(false)
    };
    let speech_status = if speech_enabled {
        "enabled"
    } else {
        "disabled"
    };
    add_message(state, MessageRole::System, "Speech");
    add_message(
        state,
        MessageRole::System,
        &format!("  STT: {}", speech_status),
    );
    add_message(
        state,
        MessageRole::System,
        &format!("  TTS: {}", speech_status),
    );
}

fn copy_last_response_shared(state: &mut crate::tui_state::ShellState<SlashDropdown>) {
    let last_response = state
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, MessageRole::Sena))
        .map(|m| m.text.clone());

    match last_response {
        Some(text) => match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&text)) {
            Ok(_) => add_message(
                state,
                MessageRole::System,
                "Last response copied to clipboard.",
            ),
            Err(e) => add_message(state, MessageRole::Warning, &format!("Copy failed: {}", e)),
        },
        None => add_message(state, MessageRole::System, "No response to copy yet."),
    }
}

fn handle_model_dir_input_shared(
    path_str: &str,
    state: &mut crate::tui_state::ShellState<SlashDropdown>,
) -> Result<()> {
    if path_str.trim().is_empty() {
        add_message(
            state,
            MessageRole::System,
            "Model directory change cancelled. Keeping current directory.",
        );
        return Ok(());
    }

    let path = PathBuf::from(path_str.trim());
    let models = model_selector::discover_models_at(&path)?;
    state.model_popup = Some(model_selector::ModelSelectorPopup::new(models));

    Ok(())
}

#[allow(dead_code)]
fn exit_command_result(command: &str) -> Option<DispatchResult> {
    match command {
        "/close" | "/quit" | "/exit" | "/q" => Some(DispatchResult::Close),
        "/shutdown" => Some(DispatchResult::Shutdown),
        _ => None,
    }
}

#[allow(dead_code)]
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

/// Unit 21: Parse and format inference errors to be more actionable
fn format_inference_error(reason: &str, model_name: Option<&str>) -> String {
    let lower = reason.to_lowercase();

    if lower.contains("failed to load model") || lower.contains("no model file") {
        if let Some(name) = model_name {
            format!(
                "Could not load model '{}'. Is the model file accessible? Check that the path is correct and the file exists.",
                name
            )
        } else {
            "Failed to load the requested model. The model file may be missing or inaccessible."
                .to_string()
        }
    } else if lower.contains("no model available") || lower.contains("no model loaded") {
        "No AI model is loaded. Use /models to select a model first.".to_string()
    } else if lower.contains("out of memory") || lower.contains("oom") {
        "Inference failed due to insufficient memory. Try a smaller model or reduce context size."
            .to_string()
    } else if lower.contains("context length exceeded") || lower.contains("context too large") {
        "Input is too long for this model's context window. Try shorter input or use a model with larger context.".to_string()
    } else {
        // Generic error with prefix for clarity
        format!("[inference error] {}", reason)
    }
}

/// Run the interactive shell. Returns the exit reason for the restart loop.
#[allow(dead_code)]
pub async fn run(runtime: Arc<Runtime>) -> Result<ShellExitReason> {
    let shell = Shell::new(Arc::clone(&runtime));
    let transport = LocalBusTransport::new(runtime.bus.subscribe_broadcast());
    let bus = runtime.bus.clone();
    let target = DispatchTarget::LocalBus(&bus);
    run_shell(transport, target, ShellMode::Local { runtime, shell }).await
}

/// Helper to create a centered rect using percentage of the available rect.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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
#[allow(dead_code)]
fn verbose_format(ev: &Event) -> Option<String> {
    match ev {
        Event::CTP(ctp_event)
            if matches!(**ctp_event, bus::events::CTPEvent::ThoughtEventTriggered(_)) =>
        {
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
    use bus::IpcPayload;

    crate::display::banner();
    crate::display::info("Connecting to Sena daemon...");

    // Connect to daemon IPC endpoint.
    let mut ipc_client = crate::ipc_client::IpcClient::connect()
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

    let mut state = crate::tui_state::ShellState::new();
    state.add_welcome_message("Connected to Sena daemon — local-first ambient AI");
    state.add_welcome_message("Type /help for commands, or chat freely.");

    // Seed current_model from SessionReady handshake.
    state.current_model = initial_model;

    let command_state = CommandRuntimeState {
        voice_enabled: runtime::config::load_or_create_config()
            .await
            .map(|c| c.speech_enabled)
            .unwrap_or(false),
        ..CommandRuntimeState::default()
    };
    let transport = IpcTransport::new(ipc_client);
    let target = DispatchTarget::Noop;
    run_shell(
        transport,
        target,
        ShellMode::Ipc {
            state,
            command_state,
        },
    )
    .await
    .map(|_| ())
}

enum ShellMode {
    Local {
        runtime: Arc<Runtime>,
        shell: Shell,
    },
    Ipc {
        state: crate::tui_state::ShellState<SlashDropdown>,
        command_state: CommandRuntimeState,
    },
}

impl ShellMode {
    fn state(&self) -> &crate::tui_state::ShellState<SlashDropdown> {
        match self {
            Self::Local { shell, .. } => &shell.state,
            Self::Ipc { state, .. } => state,
        }
    }

    fn state_mut(&mut self) -> &mut crate::tui_state::ShellState<SlashDropdown> {
        match self {
            Self::Local { shell, .. } => &mut shell.state,
            Self::Ipc { state, .. } => state,
        }
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        match self {
            Self::Local { shell, .. } => shell.render(frame),
            Self::Ipc { state, .. } => render_ipc_tui(frame, state),
        }
    }

    fn on_tick(&mut self) {
        if let Self::Local { shell, .. } = self {
            shell.handle_transparency_timeout();
        }
    }

    fn copy_last_response(&mut self) {
        match self {
            Self::Local { shell, .. } => shell.copy_last_response(),
            Self::Ipc { state, .. } => copy_last_response_shared(state),
        }
    }

    fn dismiss_actors_popup_if_open(&mut self) -> bool {
        if let Self::Local { shell, .. } = self {
            if shell.actors_popup_visible && shell.state.model_popup.is_none() {
                shell.actors_popup_visible = false;
                return true;
            }
        }
        false
    }

    async fn dispatch_input_line<T: MessageTransport>(
        &mut self,
        line: &str,
        transport: &mut T,
        target: &mut DispatchTarget<'_>,
    ) -> DispatchResult {
        match self {
            Self::Local { shell, .. } => shell.dispatch_line(line.to_string()).await,
            Self::Ipc {
                state,
                command_state,
            } => {
                if line.starts_with('/') || state.pending_model_dir_input {
                    let mut deps = CommandDeps {
                        runtime: None,
                        state,
                        command_state,
                    };
                    match dispatch_command(line, transport, target, &mut deps).await {
                        Ok(result) => result,
                        Err(e) => {
                            add_message(
                                deps.state,
                                MessageRole::Warning,
                                &format!("Command failed: {}", e),
                            );
                            DispatchResult::Continue
                        }
                    }
                } else {
                    state
                        .messages
                        .push(Message::new(MessageRole::User, line.to_string()));
                    let request_id = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(1);
                    let event = Event::Inference(InferenceEvent::InferenceRequested {
                        prompt: line.to_string(),
                        priority: Priority::High,
                        request_id,
                        source: bus::InferenceSource::UserText,
                    });
                    if let Err(e) = transport.send(target, event).await {
                        add_message(
                            state,
                            MessageRole::Warning,
                            &format!("Failed to send chat: {}", e),
                        );
                    } else {
                        state.waiting_for_inference = true;
                        state.stats.messages_sent += 1;
                    }
                    DispatchResult::Continue
                }
            }
        }
    }

    async fn handle_message(&mut self, message: DisplayMessage) -> Option<ShellExitReason> {
        match self {
            Self::Local { shell, .. } => {
                if let DisplayMessage::BusEvent(event) = message {
                    if matches!(
                        *event,
                        Event::System(bus::events::SystemEvent::ShutdownSignal)
                    ) {
                        tracing::info!("cli: ShutdownSignal received on bus — exiting shell");
                        return Some(ShellExitReason::Shutdown);
                    }
                    shell.handle_bus_event(*event).await;
                }
                None
            }
            Self::Ipc { state, .. } => {
                use bus::LineStyle;
                if let DisplayMessage::IpcPayload(payload) = message {
                    match payload {
                        IpcPayload::DisplayLine { content, style } => {
                            if matches!(style, LineStyle::Inference) {
                                state.waiting_for_inference = false;
                            }
                            let role = match style {
                                LineStyle::Error => MessageRole::Warning,
                                LineStyle::CtpThought => MessageRole::Warning,
                                LineStyle::Inference => MessageRole::Sena,
                                LineStyle::Success => MessageRole::Sena,
                                LineStyle::Normal => MessageRole::System,
                                LineStyle::Dimmed => MessageRole::System,
                                LineStyle::SystemNotice => MessageRole::System,
                            };
                            state.messages.push(Message::new(role, content));
                        }
                        IpcPayload::DaemonShutdown => {
                            state.messages.push(Message::new(
                                MessageRole::Warning,
                                "Daemon is shutting down — CLI will exit.".to_string(),
                            ));
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            return Some(ShellExitReason::Close);
                        }
                        IpcPayload::Pong | IpcPayload::Ack { .. } => {}
                        IpcPayload::Error { reason, .. } => {
                            state.waiting_for_inference = false;
                            state.messages.push(Message::new(
                                MessageRole::Warning,
                                format!("Error: {}", reason),
                            ));
                        }
                        IpcPayload::LoopStatusUpdate { loop_name, enabled } => {
                            state.loop_states.insert(loop_name, enabled);
                        }
                        IpcPayload::ModelStatusUpdate { name } => {
                            state.current_model = Some(name);
                        }
                        _ => {}
                    }
                }
                None
            }
        }
    }

    async fn handle_model_popup_enter<T: MessageTransport>(
        &mut self,
        transport: &mut T,
        target: &mut DispatchTarget<'_>,
    ) {
        let popup = self.state_mut().model_popup.take();
        let Some(popup) = popup else {
            return;
        };

        if popup.is_change_dir_selected() {
            let state = self.state_mut();
            state.pending_model_dir_input = true;
            add_message(
                state,
                MessageRole::System,
                "Enter the full path to your model directory (Enter on empty input to cancel):",
            );
            return;
        }

        let Some(selected) = popup.selected() else {
            return;
        };
        let model_name = selected.name.clone();

        match self {
            Self::Local { runtime, shell } => {
                let mut config = runtime.config.clone();
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
            Self::Ipc { state, .. } => {
                // Unit 20: Add "Loading model..." indicator
                add_message(
                    state,
                    MessageRole::System,
                    &format!("Loading model: {}...", model_name),
                );

                let event = Event::System(bus::events::SystemEvent::ConfigSetRequested {
                    key: "preferred_model".to_string(),
                    value: model_name.clone(),
                });
                if let Err(e) = transport.send(target, event).await {
                    add_message(
                        state,
                        MessageRole::Warning,
                        &format!("Failed to change model: {}", e),
                    );
                } else {
                    // Success message will be shown when ModelLoaded event arrives
                    state.current_model = Some(model_name);
                }
            }
        }
    }
}

async fn run_shell<T: MessageTransport>(
    mut transport: T,
    mut target: DispatchTarget<'_>,
    mut mode: ShellMode,
) -> Result<ShellExitReason> {
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut exit_reason = ShellExitReason::Close;

    'main: loop {
        if let Err(e) = terminal.draw(|f| mode.render(f)) {
            add_message(
                mode.state_mut(),
                MessageRole::Warning,
                &format!("Display error: {}", e),
            );
        }

        if let Some(first_press) = mode.state().ctrl_c_first_press {
            if first_press.elapsed() > Duration::from_secs(3) {
                mode.state_mut().ctrl_c_first_press = None;
            }
        }

        tokio::select! {
            maybe_msg = transport.recv() => {
                match maybe_msg {
                    Some(message) => {
                        if let Some(reason) = mode.handle_message(message).await {
                            exit_reason = reason;
                            break 'main;
                        }
                    }
                    None => {
                        if let ShellMode::Ipc { state, .. } = &mut mode {
                            state.messages.push(Message::new(MessageRole::Warning, "Daemon disconnected.".to_string()));
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                        break 'main;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                mode.on_tick();
                loop {
                    match event::poll(Duration::from_millis(0)) {
                        Ok(false) | Err(_) => break,
                        Ok(true) => {}
                    }

                    match event::read() {
                        Err(e) => {
                            add_message(mode.state_mut(), MessageRole::Warning, &format!("Input error: {}", e));
                            break;
                        }
                        Ok(event::Event::Mouse(mouse)) => {
                            match mouse.kind {
                                event::MouseEventKind::ScrollUp => {
                                    mode.state_mut().scroll_offset = mode.state().scroll_offset.saturating_add(3);
                                }
                                event::MouseEventKind::ScrollDown => {
                                    mode.state_mut().scroll_offset = mode.state().scroll_offset.saturating_sub(3);
                                }
                                _ => {}
                            }
                        }
                        Ok(event::Event::Key(key)) => {
                            if key.kind != KeyEventKind::Press {
                                continue;
                            }

                            if mode.dismiss_actors_popup_if_open() {
                                continue;
                            }

                            match (key.code, key.modifiers) {
                                (KeyCode::Char('c'), mods) if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) => {
                                    mode.copy_last_response();
                                }
                                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                    if let Some(first_press) = mode.state().ctrl_c_first_press {
                                        if first_press.elapsed() < Duration::from_secs(3) {
                                            break 'main;
                                        }
                                    } else {
                                        mode.state_mut().ctrl_c_first_press = Some(Instant::now());
                                    }
                                }
                                (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                                    mode.copy_last_response();
                                }
                                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                    break 'main;
                                }
                                (KeyCode::Up, _) if mode.state().model_popup.is_some() => {
                                    if let Some(popup) = &mut mode.state_mut().model_popup {
                                        popup.prev();
                                    }
                                }
                                (KeyCode::Down, _) if mode.state().model_popup.is_some() => {
                                    if let Some(popup) = &mut mode.state_mut().model_popup {
                                        popup.next();
                                    }
                                }
                                (KeyCode::Enter, _) if mode.state().model_popup.is_some() => {
                                    mode.handle_model_popup_enter(&mut transport, &mut target).await;
                                }
                                (KeyCode::Esc, _) if mode.state().model_popup.is_some() => {
                                    mode.state_mut().model_popup = None;
                                    add_message(mode.state_mut(), MessageRole::System, "Model selection cancelled.");
                                }
                                (KeyCode::Up, _) if mode.state().slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                    if let Some(dd) = &mut mode.state_mut().slash_dropdown {
                                        dd.prev();
                                    }
                                }
                                (KeyCode::Down, _) if mode.state().slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                    if let Some(dd) = &mut mode.state_mut().slash_dropdown {
                                        dd.next();
                                    }
                                }
                                (KeyCode::Tab, _) if mode.state().slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                    if let Some(cmd) = mode.state().slash_dropdown.as_ref().and_then(|d| d.selected_command()) {
                                        mode.state_mut().editor.input = cmd.to_string();
                                        mode.state_mut().slash_dropdown = None;
                                    }
                                }
                                (KeyCode::Enter, _) if mode.state().slash_dropdown.as_ref().is_some_and(|d| !d.is_empty()) => {
                                    if let Some(cmd) = mode.state().slash_dropdown.as_ref().and_then(|d| d.selected_command()) {
                                        let line = cmd.to_string();
                                        mode.state_mut().editor.input.clear();
                                        mode.state_mut().slash_dropdown = None;
                                        mode.state_mut().editor.push_history(&line);
                                        match mode.dispatch_input_line(&line, &mut transport, &mut target).await {
                                            DispatchResult::Continue => {}
                                            DispatchResult::Close => {
                                                exit_reason = ShellExitReason::Close;
                                                break 'main;
                                            }
                                            DispatchResult::Shutdown => {
                                                exit_reason = ShellExitReason::Shutdown;
                                                break 'main;
                                            }
                                        }
                                    }
                                }
                                (KeyCode::Esc, _) if mode.state().slash_dropdown.is_some() => {
                                    mode.state_mut().slash_dropdown = None;
                                }
                                (KeyCode::Enter, _) => {
                                    let line = mode.state().editor.input.trim().to_string();
                                    mode.state_mut().editor.input.clear();
                                    mode.state_mut().slash_dropdown = None;
                                    if !line.is_empty() {
                                        mode.state_mut().editor.push_history(&line);
                                        match mode.dispatch_input_line(&line, &mut transport, &mut target).await {
                                            DispatchResult::Continue => {}
                                            DispatchResult::Close => {
                                                exit_reason = ShellExitReason::Close;
                                                break 'main;
                                            }
                                            DispatchResult::Shutdown => {
                                                exit_reason = ShellExitReason::Shutdown;
                                                break 'main;
                                            }
                                        }
                                    }
                                }
                                (KeyCode::Backspace, _) if mode.state().model_popup.is_none() => {
                                    mode.state_mut().editor.input.pop();
                                    if mode.state().editor.input.starts_with('/') {
                                        let current_input = mode.state().editor.input.clone();
                                        if let Some(dd) = &mut mode.state_mut().slash_dropdown {
                                            dd.update(&current_input);
                                            // Unit 20: Keep dropdown open to show "no matches" if needed
                                            if dd.is_empty() && !dd.no_matches {
                                                mode.state_mut().slash_dropdown = None;
                                            }
                                        } else {
                                            let dd = SlashDropdown::from_prefix(&current_input);
                                            // Unit 20: Show dropdown even if empty to display "no matches"
                                            if !dd.is_empty() || dd.no_matches {
                                                mode.state_mut().slash_dropdown = Some(dd);
                                            }
                                        }
                                    } else {
                                        mode.state_mut().slash_dropdown = None;
                                    }
                                }
                                (KeyCode::Up, _) if mode.state().model_popup.is_none() && mode.state().slash_dropdown.is_none() => {
                                    mode.state_mut().editor.history_prev();
                                }
                                (KeyCode::Down, _) if mode.state().model_popup.is_none() && mode.state().slash_dropdown.is_none() => {
                                    mode.state_mut().editor.history_next();
                                }
                                (KeyCode::PageUp, _) if mode.state().model_popup.is_none() => {
                                    mode.state_mut().scroll_offset = mode.state().scroll_offset.saturating_add(10);
                                }
                                (KeyCode::PageDown, _) if mode.state().model_popup.is_none() => {
                                    mode.state_mut().scroll_offset = mode.state().scroll_offset.saturating_sub(10);
                                }
                                (KeyCode::Esc, _) if mode.state().model_popup.is_none() => {
                                    mode.state_mut().editor.input.clear();
                                    mode.state_mut().slash_dropdown = None;
                                }
                                (KeyCode::Char(c), mods)
                                    if !mods.contains(KeyModifiers::CONTROL)
                                        && !mods.contains(KeyModifiers::ALT)
                                        && mode.state().model_popup.is_none() => {
                                    // Unit 20: Enforce input length limit
                                    if mode.state().editor.input.len() >= MAX_INPUT_LENGTH {
                                        // Reject character — input is at max length
                                    } else {
                                        mode.state_mut().editor.input.push(c);
                                        mode.state_mut().ctrl_c_first_press = None;
                                        if mode.state().editor.input.starts_with('/') {
                                            let current_input = mode.state().editor.input.clone();
                                            if let Some(dd) = &mut mode.state_mut().slash_dropdown {
                                                dd.update(&current_input);
                                                // Unit 20: Keep dropdown open to show "no matches" if needed
                                                if dd.is_empty() && !dd.no_matches {
                                                    mode.state_mut().slash_dropdown = None;
                                                }
                                            } else {
                                                let dd = SlashDropdown::from_prefix(&current_input);
                                                // Unit 20: Show dropdown even if empty to display "no matches"
                                                if !dd.is_empty() || dd.no_matches {
                                                    mode.state_mut().slash_dropdown = Some(dd);
                                                }
                                            }
                                        } else {
                                            mode.state_mut().slash_dropdown = None;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    drop(_guard);
    drop(terminal);

    let stats = mode.state().stats.clone();
    let close_bus = match &mode {
        ShellMode::Local { runtime, .. } if exit_reason == ShellExitReason::Close => {
            Some(runtime.bus.clone())
        }
        _ => None,
    };

    display::print_session_summary(&stats);

    if let Some(bus) = close_bus {
        let _ = bus
            .broadcast(Event::System(bus::events::SystemEvent::CliSessionClosed))
            .await;
    }

    Ok(exit_reason)
}

/// Simplified TUI rendering for IPC mode.
fn render_ipc_tui(frame: &mut ratatui::Frame, state: &crate::tui_state::ShellState<SlashDropdown>) {
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
    let elapsed = state.stats.elapsed_formatted();
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
    for msg in &state.messages {
        match msg.role {
            MessageRole::User => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "\u{25b8} ",
                        Style::default()
                            .fg(Color::LightMagenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(msg.text.clone(), Style::default().fg(Color::White)),
                ]));
                lines.push(Line::from("")); // spacing
            }
            MessageRole::Sena => {
                for line in msg.text.lines() {
                    lines.push(Line::from(Span::styled(
                        line.to_owned(),
                        Style::default().fg(Color::Green),
                    )));
                }
                lines.push(Line::from("")); // spacing
            }
            MessageRole::System => {
                // Split on '\n' so multi-line responses (observation, memory) render correctly.
                for line in msg.text.split('\n') {
                    lines.push(Line::from(Span::styled(
                        line.to_owned(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            MessageRole::Warning => {
                let mut first = true;
                for line in msg.text.split('\n') {
                    if first {
                        lines.push(Line::from(vec![
                            Span::styled(
                                "\u{26a0} ",
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(line.to_owned(), Style::default().fg(Color::Yellow)),
                        ]));
                        first = false;
                    } else {
                        lines.push(Line::from(Span::styled(
                            line.to_owned(),
                            Style::default().fg(Color::Yellow),
                        )));
                    }
                }
            }
        }
    }

    // Estimate the actual rendered (visual) line count accounting for word-wrap.
    // Without this, the auto-scroll-to-bottom undershoots when long lines wrap.
    let col_width = body_chunks[0].width.saturating_sub(2).max(1) as usize;
    let total_lines = lines.len();
    let visual_lines: usize = lines
        .iter()
        .map(|l| {
            let char_len: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            if char_len == 0 {
                1
            } else {
                char_len.div_ceil(col_width)
            }
        })
        .sum();
    let _ = total_lines; // visual_lines takes precedence for scroll
    let inner_height = body_chunks[0].height.saturating_sub(2) as usize; // subtract borders
    let scroll = if state.scroll_offset == 0 {
        visual_lines.saturating_sub(inner_height)
    } else {
        visual_lines.saturating_sub(inner_height + state.scroll_offset)
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
    render_ipc_sidebar(frame, body_chunks[1], state);

    // Input area
    let prompt_line = Line::from(vec![
        Span::styled(
            " sena ",
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("\u{203a} ", Style::default().fg(Color::DarkGray)),
        Span::raw(&state.editor.input),
        Span::styled("\u{258c}", Style::default().fg(Color::LightMagenta)),
    ]);

    let status_line = if let Some(first_press) = state.ctrl_c_first_press {
        if first_press.elapsed() < Duration::from_secs(3) {
            Line::from(Span::styled(
                " Press Ctrl+C again to exit",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
        } else if state.waiting_for_inference {
            Line::from(Span::styled(
                " \u{22ef} Thinking...",
                Style::default().fg(Color::Yellow),
            ))
        } else if state.listen_mode_active {
            Line::from(Span::styled(
                " \u{1f3a4} Listening... (type /listen to stop)",
                Style::default().fg(Color::Green),
            ))
        } else {
            Line::from(Span::styled(" Ready", Style::default().fg(Color::DarkGray)))
        }
    } else if state.waiting_for_inference {
        Line::from(Span::styled(
            " \u{22ef} Thinking...",
            Style::default().fg(Color::Yellow),
        ))
    } else if state.listen_mode_active {
        Line::from(Span::styled(
            " \u{1f3a4} Listening... (type /listen to stop)",
            Style::default().fg(Color::Green),
        ))
    } else {
        Line::from(Span::styled(" Ready", Style::default().fg(Color::DarkGray)))
    };

    let hints_line = Line::from(Span::styled(
        " Tab:\u{2508}complete   \u{2191}\u{2193}:\u{2508}history   /help:\u{2508}commands",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));

    let input_border_color = if state.waiting_for_inference {
        Color::Yellow
    } else if state.listen_mode_active {
        Color::Green
    } else {
        Color::Magenta
    };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(input_border_color))
        .title(Span::styled(
            " Input ",
            Style::default()
                .fg(input_border_color)
                .add_modifier(Modifier::BOLD),
        ));
    let input_widget =
        Paragraph::new(vec![prompt_line, status_line, hints_line]).block(input_block);
    frame.render_widget(input_widget, main_chunks[2]);

    // Slash dropdown overlay
    if let Some(ref dd) = state.slash_dropdown {
        if !dd.is_empty() && state.model_popup.is_none() {
            render_slash_dropdown_overlay(frame, dd, main_chunks[2]);
        }
    }

    // Model selector popup overlay
    if let Some(ref popup) = state.model_popup {
        model_selector::render_popup(popup, frame);
    }
}

/// Render the status sidebar for IPC mode — same layout as Shell::render_sidebar().
fn render_ipc_sidebar(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &crate::tui_state::ShellState<SlashDropdown>,
) {
    let mut lines: Vec<Line> = Vec::new();

    // ── Model ─────────────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        " Model",
        Style::default()
            .fg(Color::LightMagenta)
            .add_modifier(Modifier::BOLD),
    )));
    let model = state.current_model.as_deref().unwrap_or("(selecting...)");
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
            state.stats.messages_sent.to_string(),
            Style::default().fg(Color::White),
        ),
        Span::styled(" messages  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.stats.tokens_received.to_string(),
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
        let status = state
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
        let enabled = state.loop_states.get(*loop_name).copied().unwrap_or(true);
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

    // Unit 20: Show "no matching commands" when filter is empty
    if dd.no_matches {
        let popup_height = 3u16; // border + single line
        let popup_width = 40u16.min(frame.area().width.saturating_sub(4));
        let y = input_area.y.saturating_sub(popup_height);
        let popup_area = ratatui::layout::Rect {
            x: input_area.x + 2,
            y,
            width: popup_width,
            height: popup_height,
        };
        frame.render_widget(Clear, popup_area);
        let no_match_line = Line::from(Span::styled(
            "no matching commands",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
        let para = Paragraph::new(no_match_line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        frame.render_widget(para, popup_area);
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
            // Unit 19: Show category label next to command
            ListItem::new(Line::from(vec![
                Span::styled(
                    cmd.command,
                    Style::default()
                        .fg(Color::LightMagenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("[{}]", cmd.category.label()),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
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
