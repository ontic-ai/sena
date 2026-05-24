use crate::error::CliError;
use crate::terminal_window;
use crate::theme;
use crossterm::{
    event::{KeyCode, KeyModifiers},
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
use std::time::{Duration, Instant};
use tracing::debug;

pub(crate) type AppTerminal = Terminal<CrosstermBackend<io::Stdout>>;
const CLOSE_WINDOW: Duration = Duration::from_millis(1500);

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

pub(crate) fn handle_double_ctrl_x(
    code: KeyCode,
    modifiers: KeyModifiers,
    armed_at: &mut Option<Instant>,
) -> bool {
    let now = Instant::now();
    let is_ctrl_x = matches!(code, KeyCode::Char('x') | KeyCode::Char('X'))
        && modifiers.contains(KeyModifiers::CONTROL);

    if is_ctrl_x {
        let should_close = armed_at
            .is_some_and(|armed_at| now.duration_since(armed_at) <= CLOSE_WINDOW);
        *armed_at = if should_close { None } else { Some(now) };
        return should_close;
    }

    if armed_at.is_some_and(|armed_at| now.duration_since(armed_at) > CLOSE_WINDOW) {
        *armed_at = None;
    }

    if !matches!(code, KeyCode::Null) {
        *armed_at = None;
    }

    false
}

pub(crate) fn close_hint(armed_at: Option<Instant>) -> &'static str {
    if is_close_armed(armed_at) {
        "Press Ctrl+X again to close"
    } else {
        "Ctrl+X twice to close"
    }
}

pub(crate) fn is_close_armed(armed_at: Option<Instant>) -> bool {
    armed_at.is_some_and(|armed_at| Instant::now().duration_since(armed_at) <= CLOSE_WINDOW)
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