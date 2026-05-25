use crate::error::CliError;
use crate::terminal_window;
use crate::theme;
use crossterm::{
    event::{KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ipc::IpcClient;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use serde_json::Value;
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use std::time::Instant;
use tracing::debug;

pub(crate) type AppTerminal = Terminal<CrosstermBackend<io::Stdout>>;
const CLOSE_CONFIRMATION_SECONDS: u8 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseAction {
    Ignored,
    Armed,
    Cancelled,
    Confirmed,
}

#[derive(Clone, Default)]
pub(crate) struct CloseConfirmation {
    state: Arc<CloseConfirmationState>,
}

#[derive(Default)]
struct CloseConfirmationState {
    active: AtomicBool,
    remaining_seconds: AtomicU8,
    generation: AtomicU64,
}

pub(crate) fn init_terminal() -> Result<AppTerminal, CliError> {
    if let Err(error) = terminal_window::try_resize_default_console() {
        debug!(%error, "Skipping console resize for tab window");
    }

    enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    prime_terminal(&mut terminal)?;
    Ok(terminal)
}

pub(crate) fn prime_terminal(terminal: &mut AppTerminal) -> Result<(), CliError> {
    terminal
        .autoresize()
        .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    terminal
        .clear()
        .map_err(|e| CliError::TuiRenderError(e.to_string()))
}

pub(crate) fn restore_terminal(terminal: &mut AppTerminal) -> Result<(), CliError> {
    disable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    terminal
        .show_cursor()
        .map_err(|e| CliError::TuiRenderError(e.to_string()))
}

pub(crate) fn render_header(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    daemon_status: &str,
    daemon_uptime_secs: u64,
    close_hint: Option<&str>,
) {
    let line = build_header_line(
        area.width as usize,
        label,
        daemon_status,
        daemon_uptime_secs,
        close_hint,
    );
    frame.render_widget(Paragraph::new(Line::from(Span::styled(line, theme::title_style()))), area);
}

pub(crate) fn elapsed_uptime(base_secs: u64, anchor: Instant) -> u64 {
    base_secs + anchor.elapsed().as_secs()
}

pub(crate) fn is_shutdown_event(event: &Value) -> bool {
    matches!(
        event.get("type").and_then(|value| value.as_str()),
        Some("ShutdownInitiated" | "ShutdownRequested" | "ShutdownSignal")
    )
}

pub(crate) fn watch_daemon_connection(ipc: &IpcClient) -> Arc<AtomicBool> {
    let connection_alive = Arc::new(AtomicBool::new(true));
    let connection_flag = Arc::clone(&connection_alive);
    let mut push_rx = ipc.subscribe_events();

    tokio::spawn(async move {
        while let Some(event) = push_rx.recv().await {
            if is_shutdown_event(&event) {
                break;
            }
        }

        connection_flag.store(false, Ordering::SeqCst);
    });

    connection_alive
}

impl CloseConfirmation {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn handle_key(&self, code: KeyCode, modifiers: KeyModifiers) -> CloseAction {
        let is_ctrl_x = matches!(code, KeyCode::Char('x') | KeyCode::Char('X'))
            && modifiers.contains(KeyModifiers::CONTROL);

        if is_ctrl_x {
            if self.is_active() {
                self.clear();
                return CloseAction::Confirmed;
            }

            self.arm();
            return CloseAction::Armed;
        }

        if self.is_active() {
            self.clear();
            return CloseAction::Cancelled;
        }

        CloseAction::Ignored
    }

    pub(crate) fn is_active(&self) -> bool {
        self.state.active.load(Ordering::SeqCst)
    }

    pub(crate) fn remaining_seconds(&self) -> u8 {
        self.state.remaining_seconds.load(Ordering::SeqCst)
    }

    fn arm(&self) {
        let generation = self.state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.state.active.store(true, Ordering::SeqCst);
        self.state
            .remaining_seconds
            .store(CLOSE_CONFIRMATION_SECONDS, Ordering::SeqCst);

        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + tokio::time::Duration::from_secs(1),
                tokio::time::Duration::from_secs(1),
            );
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            for remaining in (1..CLOSE_CONFIRMATION_SECONDS).rev() {
                interval.tick().await;
                if state.generation.load(Ordering::SeqCst) != generation {
                    return;
                }

                state.remaining_seconds.store(remaining, Ordering::SeqCst);
            }

            interval.tick().await;
            if state.generation.load(Ordering::SeqCst) == generation {
                state.active.store(false, Ordering::SeqCst);
                state.remaining_seconds.store(0, Ordering::SeqCst);
            }
        });
    }

    fn clear(&self) {
        self.state.generation.fetch_add(1, Ordering::SeqCst);
        self.state.active.store(false, Ordering::SeqCst);
        self.state.remaining_seconds.store(0, Ordering::SeqCst);
    }
}

pub(crate) fn close_hint() -> &'static str {
    "Ctrl+X twice to close"
}

pub(crate) fn render_close_confirmation(frame: &mut Frame, remaining_seconds: u8) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().borders(Borders::ALL), area);

    let content_area = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(content_area);

    frame.render_widget(
        Paragraph::new("Close this window?").alignment(Alignment::Center),
        rows[1],
    );

    let number_area = centered_rect(rows[3], 5, 3);
    frame.render_widget(Block::default().borders(Borders::ALL), number_area);
    frame.render_widget(
        Paragraph::new(remaining_seconds.to_string()).alignment(Alignment::Center),
        number_area,
    );

    frame.render_widget(
        Paragraph::new("Press Ctrl+X again to confirm close").alignment(Alignment::Center),
        rows[5],
    );
    frame.render_widget(
        Paragraph::new("Press any other key to cancel").alignment(Alignment::Center),
        rows[6],
    );
}

fn build_header_line(
    width: usize,
    label: &str,
    daemon_status: &str,
    daemon_uptime_secs: u64,
    close_hint: Option<&str>,
) -> String {
    let mut content = format!(
        "─── SENA [{}] ─── {} · {}",
        label,
        daemon_status,
        format_uptime(daemon_uptime_secs)
    );

    if let Some(close_hint) = close_hint {
        content.push_str(" ─── ");
        content.push_str(close_hint);
    }

    if width > content.chars().count() {
        content.push_str(&"─".repeat(width - content.chars().count()));
    }

    content
}

fn format_uptime(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;

    if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(width.min(area.width)),
            Constraint::Min(0),
        ])
        .split(vertical[1]);
    horizontal[1]
}