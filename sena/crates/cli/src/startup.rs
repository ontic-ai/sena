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
struct ActorSelectionStatusResponse {
    actor_selection_pending: bool,
    actors: Vec<ActorSelectionActor>,
    #[serde(default)]
    selected_actors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ActorSelectionActor {
    id: String,
    display_name: String,
    description: String,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    can_start_without_dependencies: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Confirm,
    Actors,
}

struct ActorSelectionApp {
    actors: Vec<ActorSelectionActor>,
    selected_ids: BTreeSet<String>,
    cursor: usize,
    focus: Focus,
}

pub struct StartupSelection {
    pub current_selected_ids: Vec<String>,
    pub selected_ids: Vec<String>,
}

impl ActorSelectionApp {
    fn new(actors: Vec<ActorSelectionActor>, selected_ids: Vec<String>) -> Self {
        Self {
            actors,
            selected_ids: selected_ids.into_iter().collect(),
            cursor: 0,
            focus: Focus::Confirm,
        }
    }

    fn run(mut self) -> Result<Vec<String>, CliError> {
        if let Err(error) = terminal_window::try_resize_default_console() {
            debug!(%error, "Skipping console resize for actor selection");
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
            tab_chrome::sync_terminal_before_draw(&mut terminal)
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
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
                    KeyCode::Char(' ') if self.focus == Focus::Actors => {
                        self.toggle_current_actor();
                    }
                    KeyCode::Enter => {
                        if self.focus == Focus::Confirm {
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
            Focus::Confirm => {
                self.focus = Focus::Actors;
                self.cursor = self.actors.len().saturating_sub(1);
            }
            Focus::Actors => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Confirm => {
                if !self.actors.is_empty() {
                    self.focus = Focus::Actors;
                    self.cursor = 0;
                }
            }
            Focus::Actors => {
                if self.cursor + 1 < self.actors.len() {
                    self.cursor += 1;
                } else {
                    self.focus = Focus::Confirm;
                }
            }
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Confirm => Focus::Actors,
            Focus::Actors => Focus::Confirm,
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

    fn missing_dependency(&self, actor_id: &str) -> Option<&ActorSelectionActor> {
        let actor = self.actors.iter().find(|actor| actor.id == actor_id)?;
        if actor.can_start_without_dependencies {
            return None;
        }

        actor
            .dependencies
            .iter()
            .find(|dependency| !self.selected_ids.contains(dependency.as_str()))
            .and_then(|dependency| self.actors.iter().find(|actor| actor.id == *dependency))
    }

    fn dependency_names(&self, actor: &ActorSelectionActor) -> String {
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
                Constraint::Length(4),
                Constraint::Min(0),
                Constraint::Length(4),
            ])
            .split(frame.area());

        let header = Paragraph::new(vec![
            Line::from(Span::styled(
                "Sena Startup Actor Selection",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Press Enter to continue with the current actors, or move down to change selection.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "Arrow keys navigate. Space or Enter toggles an actor while focused in the list.",
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

        let button_style = if self.focus == Focus::Confirm {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let footer = Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    "Current selection starts immediately ->  ",
                    Style::default().fg(Color::Gray),
                ),
                Span::styled("[ Continue ]", button_style),
            ]),
            Line::from(Span::styled(
                format!("{} actor(s) selected", self.selected_ids.len()),
                Style::default().fg(Color::Gray),
            )),
        ])
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, layout[2]);
    }
}

pub async fn choose_actor_selection(ipc: &mut IpcClient) -> Result<StartupSelection, CliError> {
    let response = ipc
        .send("runtime.actor_selection_status", json!({}))
        .await
        .map_err(CliError::Ipc)?;
    let status: ActorSelectionStatusResponse = serde_json::from_value(response).map_err(|e| {
        CliError::IpcReceiveError(format!("invalid actor selection response: {}", e))
    })?;

    let current_selected_ids = if status.selected_actors.is_empty() {
        status.actors.iter().map(|actor| actor.id.clone()).collect()
    } else {
        status.selected_actors
    };
    let selected_ids = ActorSelectionApp::new(status.actors, current_selected_ids.clone()).run()?;

    Ok(StartupSelection {
        current_selected_ids,
        selected_ids,
    })
}

pub async fn submit_pending_actor_selection(
    ipc: &mut IpcClient,
    selected_ids: &[String],
) -> Result<bool, CliError> {
    let response = ipc
        .send("runtime.actor_selection_status", json!({}))
        .await
        .map_err(CliError::Ipc)?;
    let status: ActorSelectionStatusResponse = serde_json::from_value(response).map_err(|e| {
        CliError::IpcReceiveError(format!("invalid actor selection response: {}", e))
    })?;

    if !status.actor_selection_pending {
        return Ok(false);
    }

    ipc.send(
        "runtime.submit_actor_selection",
        json!({
            "actors": selected_ids,
        }),
    )
    .await
    .map_err(CliError::Ipc)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: &str, display_name: &str, dependencies: &[&str]) -> ActorSelectionActor {
        ActorSelectionActor {
            id: id.to_string(),
            display_name: display_name.to_string(),
            description: format!("{} description", display_name),
            dependencies: dependencies.iter().map(|dependency| dependency.to_string()).collect(),
            can_start_without_dependencies: false,
        }
    }

    fn actor_graph() -> Vec<ActorSelectionActor> {
        vec![
            actor("soul", "Soul & Identity", &[]),
            actor("inference", "Inference (LLM)", &[]),
            actor("memory", "Memory", &["inference"]),
            actor("platform", "Platform Sensing", &[]),
            actor("ctp", "Thought Processing (CTP)", &["platform", "inference"]),
            actor("prompt", "Prompt Assembly", &["inference"]),
            actor("stt", "Speech Input (STT)", &[]),
            actor("tts", "Speech Output (TTS)", &[]),
            actor("sri", "Runtime Interface (SRI)", &[]),
        ]
    }

    #[test]
    fn disabling_dependency_clears_transitive_dependents() {
        let actors = actor_graph();
        let mut app = ActorSelectionApp::new(
            actors.clone(),
            actors.iter().map(|actor| actor.id.clone()).collect(),
        );
        app.focus = Focus::Actors;
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
    fn app_defaults_to_confirm_focus_for_enter_through_startup() {
        let actors = actor_graph();
        let app = ActorSelectionApp::new(
            actors.clone(),
            actors.iter().map(|actor| actor.id.clone()).collect(),
        );

        assert_eq!(app.focus, Focus::Confirm);
    }
}