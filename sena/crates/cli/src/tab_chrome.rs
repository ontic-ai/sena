use crate::error::CliError;
use crate::terminal_window;
use crate::theme;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
};
use std::io;
use std::time::Instant;
use tracing::debug;

pub(crate) type AppTerminal = Terminal<CrosstermBackend<io::Stdout>>;

pub(crate) fn init_terminal() -> Result<AppTerminal, CliError> {
    if let Err(error) = terminal_window::try_resize_default_console() {
        debug!(%error, "Skipping console resize for tab window");
    }

    enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| CliError::TuiRenderError(e.to_string()))?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))
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