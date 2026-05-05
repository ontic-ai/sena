use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders},
};

const ACCENT: Color = Color::Rgb(94, 190, 173);
const TEXT: Color = Color::Rgb(232, 225, 201);
const MUTED: Color = Color::Rgb(144, 151, 142);
const SUCCESS: Color = Color::Rgb(142, 192, 124);
const WARNING: Color = Color::Rgb(250, 189, 47);
const DANGER: Color = Color::Rgb(251, 73, 52);
const SELECTION_BG: Color = Color::Rgb(55, 77, 74);
const PANEL_BORDER: Color = Color::Rgb(74, 89, 88);

pub fn panel<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PANEL_BORDER))
        .title(title)
        .title_style(title_style())
}

pub fn focused_panel<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(title)
        .title_style(title_style())
}

pub fn title_style() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn text() -> Style {
    Style::default().fg(TEXT)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn success() -> Style {
    Style::default().fg(SUCCESS).add_modifier(Modifier::BOLD)
}

pub fn warning() -> Style {
    Style::default().fg(WARNING).add_modifier(Modifier::BOLD)
}

pub fn danger() -> Style {
    Style::default().fg(DANGER).add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    text().bg(SELECTION_BG).add_modifier(Modifier::BOLD)
}

pub fn readonly() -> Style {
    muted().add_modifier(Modifier::DIM)
}

pub fn log_line(message: &str) -> Style {
    if message.starts_with("[ERR]") {
        danger()
    } else if message.starts_with("[unclear]") {
        danger()
    } else if message.starts_with("[STT~]") {
        muted()
    } else if message.starts_with("[STT]") || message.starts_with("[TTS]") {
        text()
    } else if message.starts_with("[SYS]") || message.starts_with("[INF]") {
        success()
    } else if message.starts_with("[MEM]") || message.starts_with("[PLT]") {
        warning()
    } else if message.starts_with('>') {
        title_style()
    } else {
        text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_line_styles_voice_transcripts_by_state() {
        assert_eq!(log_line("[STT~] hello"), muted());
        assert_eq!(log_line("[STT] \"hello world\""), text());
        assert_eq!(log_line("[unclear] \"maybe\" (conf: 0.41)"), danger());
    }
}
