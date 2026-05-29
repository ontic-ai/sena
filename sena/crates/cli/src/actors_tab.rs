use crate::daemon_client::{connect_to_daemon, start_daemon, wait_for_runtime_ready};
use crate::error::CliError;
use crate::tab_chrome;
use crate::theme;
use bus::events::system::{ActorHealth, ActorStatus};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ipc::IpcClient;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeSet, HashMap};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
struct TestModeStatusResponse {
    pending: bool,
    actors: Vec<TestModeActor>,
    #[serde(default)]
    selected_actors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct TestModeActor {
    id: String,
    display_name: String,
    description: String,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    can_start_without_dependencies: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct RuntimeStatusResponse {
    #[serde(default)]
    uptime_seconds: u64,
    #[serde(default)]
    actors: Vec<ActorHealth>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Actors,
    Restart,
}

enum ActorsOutcome {
    Close,
    Restart(Vec<String>),
}

struct ActorsTab {
    terminal: tab_chrome::AppTerminal,
    actors: Vec<TestModeActor>,
    selected_ids: BTreeSet<String>,
    health_by_id: HashMap<String, ActorStatus>,
    cursor: usize,
    focus: Focus,
    status_line: String,
    daemon_uptime_secs: u64,
    daemon_uptime_anchor: Instant,
    close_confirmation: tab_chrome::CloseConfirmation,
    connection_alive: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ActorsRenderState {
    actors: Vec<TestModeActor>,
    selected_ids: BTreeSet<String>,
    health_by_id: HashMap<String, ActorStatus>,
    cursor: usize,
    focus: Focus,
    status_line: String,
}

pub async fn run(mut ipc: IpcClient) -> Result<(), CliError> {
    loop {
        let mut tab = ActorsTab::new(&mut ipc).await?;
        match tab.run_loop()? {
            ActorsOutcome::Close => {
                tab.cleanup()?;
                return Ok(());
            }
            ActorsOutcome::Restart(selected_ids) => {
                tab.cleanup()?;
                ipc = restart_selected_actors(selected_ids).await?;
            }
        }
    }
}

impl ActorsTab {
    async fn new(ipc: &mut IpcClient) -> Result<Self, CliError> {
        let terminal = tab_chrome::init_terminal()?;

        let test_mode: TestModeStatusResponse = serde_json::from_value(
            ipc.send("runtime.test_mode_status", json!({})).await?,
        )
        .map_err(|e| CliError::IpcReceiveError(e.to_string()))?;

        let runtime_status = ipc
            .send("runtime.status", json!({}))
            .await
            .ok()
            .and_then(|value| serde_json::from_value::<RuntimeStatusResponse>(value).ok())
            .unwrap_or(RuntimeStatusResponse {
                uptime_seconds: 0,
                actors: Vec::new(),
            });

        let selected_ids = if test_mode.selected_actors.is_empty() {
            test_mode.actors.iter().map(|actor| actor.id.clone()).collect()
        } else {
            test_mode.selected_actors.iter().cloned().collect()
        };
        let health_by_id = runtime_status
            .actors
            .into_iter()
            .map(|actor| (actor.name, actor.status))
            .collect();

        Ok(Self {
            terminal,
            actors: test_mode.actors,
            selected_ids,
            health_by_id,
            cursor: 0,
            focus: Focus::Actors,
            status_line: if test_mode.pending {
                "Daemon is waiting for a selection before boot continues.".to_string()
            } else {
                "Select actors, then move to Restart Selected Actors.".to_string()
            },
            daemon_uptime_secs: runtime_status.uptime_seconds,
            daemon_uptime_anchor: Instant::now(),
            close_confirmation: tab_chrome::CloseConfirmation::new(),
            connection_alive: tab_chrome::watch_daemon_connection(ipc),
        })
    }

    fn run_loop(&mut self) -> Result<ActorsOutcome, CliError> {
        loop {
            if !self.connection_alive.load(Ordering::SeqCst) {
                return Ok(ActorsOutcome::Close);
            }

            self.render()?;

            if event::poll(Duration::from_millis(100))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && let Event::Key(key) =
                    event::read().map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && key.kind == KeyEventKind::Press
            {
                match self.close_confirmation.handle_key(key.code, key.modifiers) {
                    tab_chrome::CloseAction::Confirmed => return Ok(ActorsOutcome::Close),
                    tab_chrome::CloseAction::Armed
                    | tab_chrome::CloseAction::Cancelled => continue,
                    tab_chrome::CloseAction::Ignored => {}
                }

                match key.code {
                    KeyCode::Up => self.move_up(),
                    KeyCode::Down => self.move_down(),
                    KeyCode::Tab => self.toggle_focus(),
                    KeyCode::Char(' ') => self.toggle_current_actor(),
                    KeyCode::Enter => {
                        if self.focus == Focus::Restart {
                            return Ok(ActorsOutcome::Restart(self.selected_ids_in_order()));
                        }
                        self.toggle_current_actor();
                    }
                    _ => {}
                }
            }
        }
    }

    fn cleanup(&mut self) -> Result<(), CliError> {
        tab_chrome::restore_terminal(&mut self.terminal)
    }

    fn render(&mut self) -> Result<(), CliError> {
        let daemon_uptime_secs =
            tab_chrome::elapsed_uptime(self.daemon_uptime_secs, self.daemon_uptime_anchor);
        let render_state = ActorsRenderState {
            actors: self.actors.clone(),
            selected_ids: self.selected_ids.clone(),
            health_by_id: self.health_by_id.clone(),
            cursor: self.cursor,
            focus: self.focus,
            status_line: self.status_line.clone(),
        };

        tab_chrome::sync_terminal_before_draw(&mut self.terminal)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        self.terminal
            .draw(|frame| {
                Self::render_frame(
                    frame,
                    &render_state,
                    daemon_uptime_secs,
                    &self.close_confirmation,
                )
            })
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        Ok(())
    }

    fn render_frame(
        frame: &mut Frame,
        state: &ActorsRenderState,
        daemon_uptime_secs: u64,
        close_confirmation: &tab_chrome::CloseConfirmation,
    ) {
        if close_confirmation.is_active() {
            tab_chrome::render_close_confirmation(frame, close_confirmation.remaining_seconds());
            return;
        }

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(2)])
            .split(frame.area());
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(54), Constraint::Percentage(46)])
            .split(vertical[1]);

        tab_chrome::render_header(
            frame,
            vertical[0],
            "ACTORS",
            "Connected",
            daemon_uptime_secs,
            Some(tab_chrome::close_hint()),
        );

        let items: Vec<ListItem> = state
            .actors
            .iter()
            .enumerate()
            .map(|(index, actor)| {
                let style = if index == state.cursor && state.focus == Focus::Actors {
                    theme::selected()
                } else {
                    theme::text()
                };
                let health = state
                    .health_by_id
                    .get(actor.id.as_str())
                    .map(actor_badge)
                    .unwrap_or("[?]");

                ListItem::new(Line::from(vec![
                    Span::styled(Self::checkbox(&state.selected_ids, actor.id.as_str()), style),
                    Span::styled(" ", style),
                    Span::styled(health, style),
                    Span::styled(" ", style),
                    Span::styled(actor.display_name.clone(), style),
                ]))
            })
            .collect();

        frame.render_widget(
            List::new(items).block(theme::panel("Selectable Actors")),
            body[0],
        );

        frame.render_widget(
            Paragraph::new(Self::detail_lines(state))
                .block(theme::panel("Selection Details"))
                .wrap(Wrap { trim: false }),
            body[1],
        );

        frame.render_widget(
            Paragraph::new(Line::from(state.status_line.clone())).block(theme::panel("Status")),
            vertical[2],
        );
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Actors => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            Focus::Restart => {
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
                    self.focus = Focus::Restart;
                }
            }
            Focus::Restart => {}
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Actors => Focus::Restart,
            Focus::Restart => Focus::Actors,
        };
    }

    fn toggle_current_actor(&mut self) {
        let Some(actor) = self.actors.get(self.cursor).cloned() else {
            return;
        };

        if let Some(missing) = self.missing_dependency(&actor) {
            self.status_line = format!(
                "Select '{}' before enabling '{}'.",
                missing.display_name, actor.display_name
            );
            return;
        }

        if self.selected_ids.remove(actor.id.as_str()) {
            for dependent in self.transitive_dependents(actor.id.as_str()) {
                self.selected_ids.remove(dependent.as_str());
            }
            self.status_line = format!("Disabled '{}' and dependent actors.", actor.display_name);
        } else {
            self.selected_ids.insert(actor.id.clone());
            self.status_line = format!("Enabled '{}'.", actor.display_name);
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

    fn missing_dependency<'a>(&'a self, actor: &'a TestModeActor) -> Option<&'a TestModeActor> {
        if actor.can_start_without_dependencies {
            return None;
        }

        actor
            .dependencies
            .iter()
            .find(|dependency| !self.selected_ids.contains(dependency.as_str()))
            .and_then(|dependency| self.actors.iter().find(|candidate| candidate.id == *dependency))
    }

    fn checkbox(selected_ids: &BTreeSet<String>, actor_id: &str) -> &'static str {
        if selected_ids.contains(actor_id) {
            "[x]"
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

    fn detail_lines(state: &ActorsRenderState) -> Vec<Line<'static>> {
        let actor = state.actors.get(state.cursor);
        let mut lines = if let Some(actor) = actor {
            let dependencies = if actor.dependencies.is_empty() {
                "(none)".to_string()
            } else {
                actor.dependencies.join(", ")
            };
            let health = state
                .health_by_id
                .get(actor.id.as_str())
                .map(actor_status_label)
                .unwrap_or_else(|| "unknown".to_string());

            vec![
                Line::from(actor.display_name.clone()),
                Line::from(""),
                Line::from(format!("status: {}", health)),
                Line::from(format!("dependencies: {}", dependencies)),
                Line::from(format!("selection id: {}", actor.id)),
                Line::from(""),
                Line::from(actor.description.clone()),
                Line::from(""),
            ]
        } else {
            vec![Line::from("No actor metadata available.")]
        };

        let restart_label = if state.focus == Focus::Restart {
            "> Restart Selected Actors <"
        } else {
            "Restart Selected Actors"
        };
        lines.push(Line::from(restart_label));
        lines.push(Line::from(format!(
            "selected: {} actor(s)",
            state.selected_ids.len()
        )));
        lines.push(Line::from("Tab switches focus. Enter applies the current action."));
        lines
    }
}

async fn restart_selected_actors(selected_ids: Vec<String>) -> Result<IpcClient, CliError> {
    if selected_ids.is_empty() {
        return Err(CliError::ShellRunError(
            "select at least one actor before restarting".to_string(),
        ));
    }

    let mut ipc = connect_to_daemon().await?;
    ipc.send("runtime.test_mode_restart", json!({})).await?;

    for _ in 0..100 {
        if !IpcClient::daemon_running().await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    start_daemon(true)?;
    let mut replacement = connect_to_daemon().await?;
    replacement
        .send(
            "runtime.boot_with_selection",
            json!({
                "actors": selected_ids,
            }),
        )
        .await?;
    wait_for_runtime_ready(&mut replacement).await?;
    Ok(replacement)
}

fn actor_badge(status: &ActorStatus) -> &'static str {
    match status {
        ActorStatus::Starting => "[~]",
        ActorStatus::Ready => "[+]",
        ActorStatus::Idle => "[-]",
        ActorStatus::Degraded { .. } => "[!]",
        ActorStatus::Failed { .. } => "[x]",
    }
}

fn actor_status_label(status: &ActorStatus) -> String {
    match status {
        ActorStatus::Starting => "starting".to_string(),
        ActorStatus::Ready => "ready".to_string(),
        ActorStatus::Idle => "idle".to_string(),
        ActorStatus::Degraded { reason } => format!("degraded: {}", reason),
        ActorStatus::Failed { reason } => format!("failed: {}", reason),
    }
}