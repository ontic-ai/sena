use crate::{error::CliError, tab_chrome, terminal_window};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ipc::IpcClient;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;
use tracing::debug;

#[derive(Clone, Debug, Deserialize)]
struct TestModeStatusResponse {
    pending: bool,
    actors: Vec<TestModeActor>,
}

#[derive(Clone, Debug, Deserialize)]
struct TestModeActor {
    id: String,
    display_name: String,
    description: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Actors,
    Start,
}

struct TestModeApp {
    actors: Vec<TestModeActor>,
    selected_ids: BTreeSet<String>,
    cursor: usize,
    focus: Focus,
}

impl TestModeApp {
    fn new(actors: Vec<TestModeActor>) -> Self {
        let selected_ids = actors.iter().map(|actor| actor.id.clone()).collect();
        Self {
            actors,
            selected_ids,
            cursor: 0,
            focus: Focus::Actors,
        }
    }

    fn run(mut self) -> Result<Vec<String>, CliError> {
        if let Err(error) = terminal_window::try_resize_default_console() {
            debug!(%error, "Skipping console resize for test mode");
        }

        enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        tab_chrome::prime_terminal(&mut terminal)?;

        let result = loop {
            terminal
                .draw(|frame| self.render(frame))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?;

            if event::poll(Duration::from_millis(100))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && let Event::Key(key) =
                    event::read().map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Up => self.move_up(),
                    KeyCode::Down => self.move_down(),
                    KeyCode::Tab => self.toggle_focus(),
                    KeyCode::Enter => {
                        if self.focus == Focus::Start {
                            break Ok(self.selected_ids_in_order());
                        }
                        self.toggle_current_actor();
                    }
                    _ => {}
                }
            }
        };

        disable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        terminal
            .show_cursor()
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        result
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Actors => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            Focus::Start => {
                self.focus = Focus::Actors;
                self.cursor = self.actors.len().saturating_sub(1);
            }
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Actors => {
                if self.cursor + 1 < self.actors.len() {
                    self.cursor += 1;
                } else {
                    self.focus = Focus::Start;
                }
            }
            Focus::Start => {}
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Actors => Focus::Start,
            Focus::Start => Focus::Actors,
        };
    }

    fn toggle_current_actor(&mut self) {
        let Some(actor) = self.actors.get(self.cursor) else {
            return;
        };

        if self.missing_dependency(actor.id.as_str()).is_some() {
            return;
        }

        if self.selected_ids.remove(actor.id.as_str()) {
            for dependent_id in self.transitive_dependents(actor.id.as_str()) {
                self.selected_ids.remove(dependent_id.as_str());
            }
        } else {
            self.selected_ids.insert(actor.id.clone());
        }
    }

    fn transitive_dependents(&self, actor_id: &str) -> Vec<String> {
        let mut pending = vec![actor_id.to_string()];
        let mut visited = BTreeSet::new();
        let mut dependents = Vec::new();

        while let Some(current) = pending.pop() {
            for actor in &self.actors {
                if actor.dependencies.iter().any(|dependency| dependency == &current)
                    && visited.insert(actor.id.clone())
                {
                    dependents.push(actor.id.clone());
                    pending.push(actor.id.clone());
                }
            }
        }

        dependents
    }

    fn missing_dependency(&self, actor_id: &str) -> Option<&TestModeActor> {
        let actor = self.actors.iter().find(|actor| actor.id == actor_id)?;
        actor
            .dependencies
            .iter()
            .find(|dependency| !self.selected_ids.contains(dependency.as_str()))
            .and_then(|dependency| self.actors.iter().find(|actor| actor.id == *dependency))
    }

    fn dependency_names(&self, actor: &TestModeActor) -> String {
        actor
            .dependencies
            .iter()
            .filter_map(|dependency| {
                self.actors
                    .iter()
                    .find(|candidate| candidate.id == *dependency)
                    .map(|candidate| candidate.display_name.clone())
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn checkbox(&self, actor_id: &str) -> &'static str {
        if self.missing_dependency(actor_id).is_some() {
            "[~]"
        } else if self.selected_ids.contains(actor_id) {
            "[✓]"
        } else {
            "[ ]"
        }
    }

    fn selected_ids_in_order(&self) -> Vec<String> {
        self.actors
            .iter()
            .filter(|actor| self.selected_ids.contains(actor.id.as_str()))
            .map(|actor| actor.id.clone())
            .collect()
    }

    fn render(&self, frame: &mut Frame) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(frame.area());

        let header = Paragraph::new(vec![
            Line::from(Span::styled(
                "SENA TEST MODE - Select which actors to run this session",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Arrow keys to navigate - Enter to toggle - Tab to confirm",
                Style::default().fg(Color::Gray),
            )),
        ])
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, layout[0]);

        let mut lines = Vec::new();
        for (index, actor) in self.actors.iter().enumerate() {
            let missing_dependency = self.missing_dependency(actor.id.as_str());
            let is_focused = self.focus == Focus::Actors && self.cursor == index;
            let base_style = if missing_dependency.is_some() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };
            let title_style = if is_focused {
                base_style.add_modifier(Modifier::BOLD).bg(Color::DarkGray)
            } else {
                base_style
            };

            lines.push(Line::from(Span::styled(
                format!("  {} {}", self.checkbox(actor.id.as_str()), actor.display_name),
                title_style,
            )));
            lines.push(Line::from(Span::styled(
                format!("      {}", actor.description),
                if missing_dependency.is_some() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Gray)
                },
            )));

            if let Some(missing_dependency) = missing_dependency {
                lines.push(Line::from(Span::styled(
                    format!(
                        "      (disabled - requires {})",
                        missing_dependency.display_name
                    ),
                    Style::default().fg(Color::DarkGray),
                )));
            } else if !actor.dependencies.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("      (requires: {})", self.dependency_names(actor)),
                    Style::default().fg(Color::Gray),
                )));
            }

            lines.push(Line::from(""));
        }

        let body = Paragraph::new(lines)
            .block(Block::default().borders(Borders::LEFT | Borders::RIGHT))
            .wrap(Wrap { trim: false });
        frame.render_widget(body, layout[1]);

        let button_style = if self.focus == Focus::Start {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(
                "Tab / Enter on this bar to start ->  ",
                Style::default().fg(Color::Gray),
            ),
            Span::styled("[ Start Sena ]", button_style),
        ]))
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, layout[2]);
    }
}

pub async fn complete_pending_selection(ipc: &mut IpcClient) -> Result<bool, CliError> {
    let response = ipc
        .send("runtime.test_mode_status", json!({}))
        .await
        .map_err(CliError::Ipc)?;
    let status: TestModeStatusResponse = serde_json::from_value(response)
        .map_err(|e| CliError::IpcReceiveError(format!("invalid test mode response: {}", e)))?;

    if !status.pending {
        return Ok(false);
    }

    let selected_actors = TestModeApp::new(status.actors).run()?;
    ipc.send(
        "runtime.boot_with_selection",
        json!({
            "actors": selected_actors,
        }),
    )
    .await
    .map_err(CliError::Ipc)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: &str, display_name: &str, dependencies: &[&str]) -> TestModeActor {
        TestModeActor {
            id: id.to_string(),
            display_name: display_name.to_string(),
            description: format!("{} description", display_name),
            dependencies: dependencies.iter().map(|dependency| dependency.to_string()).collect(),
        }
    }

    fn actor_graph() -> Vec<TestModeActor> {
        vec![
            actor("soul", "Soul & Identity", &[]),
            actor("inference", "Inference (LLM)", &[]),
            actor("memory", "Memory", &["inference"]),
            actor("platform", "Platform Sensing", &[]),
            actor("ctp", "Thought Processing (CTP)", &["platform", "inference"]),
            actor("prompt", "Prompt Assembly", &["inference", "soul"]),
            actor("stt", "Speech Input (STT)", &[]),
            actor("tts", "Speech Output (TTS)", &[]),
            actor("sri", "Runtime Interface (SRI)", &[]),
        ]
    }

    #[test]
    fn disabling_dependency_clears_transitive_dependents() {
        let mut app = TestModeApp::new(actor_graph());
        app.cursor = 1;

        app.toggle_current_actor();

        assert!(!app.selected_ids.contains("inference"));
        assert!(!app.selected_ids.contains("memory"));
        assert!(!app.selected_ids.contains("ctp"));
        assert!(!app.selected_ids.contains("prompt"));
        assert_eq!(
            app.missing_dependency("memory")
                .map(|actor| actor.display_name.as_str()),
            Some("Inference (LLM)")
        );
    }

    #[test]
    fn reenabling_dependency_leaves_dependents_unchecked() {
        let mut app = TestModeApp::new(actor_graph());
        app.cursor = 1;

        app.toggle_current_actor();
        app.toggle_current_actor();

        assert!(app.selected_ids.contains("inference"));
        assert!(!app.selected_ids.contains("memory"));
        assert!(!app.selected_ids.contains("ctp"));
        assert!(!app.selected_ids.contains("prompt"));
        assert!(app.missing_dependency("memory").is_none());
        assert_eq!(app.checkbox("memory"), "[ ]");
    }
}