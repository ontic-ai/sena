use crate::error::CliError;
use crate::tab_chrome;
use crate::theme;
use crossterm::event::{self, Event, KeyEventKind};
use ipc::IpcClient;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use serde_json::json;
use sri::{HealthStatus, RegisteredSriNode, SriEvent, SriResourceSnapshot, SriSnapshot};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct ResourceState {
    latest_resources: Option<SriResourceSnapshot>,
    nodes: Vec<RegisteredSriNode>,
    history: VecDeque<String>,
    alerts: VecDeque<String>,
}

impl ResourceState {
    fn record_snapshot(&mut self, snapshot: SriResourceSnapshot) {
        self.history.push_back(format_resource_sample(&snapshot));
        while self.history.len() > 12 {
            self.history.pop_front();
        }
        self.latest_resources = Some(snapshot);
    }

    fn push_alert(&mut self, alert: String) {
        self.alerts.push_back(alert);
        while self.alerts.len() > 8 {
            self.alerts.pop_front();
        }
    }
}

struct ResourcesTab {
    terminal: tab_chrome::AppTerminal,
    state: Arc<Mutex<ResourceState>>,
    connection_alive: Arc<AtomicBool>,
    daemon_uptime_secs: u64,
    daemon_uptime_anchor: Instant,
    close_confirmation: tab_chrome::CloseConfirmation,
}

pub async fn run(mut ipc: IpcClient) -> Result<(), CliError> {
    let mut tab = ResourcesTab::new(&mut ipc).await?;
    let result = tab.run_loop();
    let cleanup_result = tab.cleanup();
    result.and(cleanup_result)
}

impl ResourcesTab {
    async fn new(ipc: &mut IpcClient) -> Result<Self, CliError> {
        let terminal = tab_chrome::init_terminal()?;
        let daemon_uptime_secs = ipc
            .send("runtime.ping", json!({}))
            .await
            .ok()
            .and_then(|response| response.get("uptime_seconds").and_then(|value| value.as_u64()))
            .unwrap_or(0);

        let state = Arc::new(Mutex::new(ResourceState {
            latest_resources: None,
            nodes: Vec::new(),
            history: VecDeque::new(),
            alerts: VecDeque::new(),
        }));

        let _ = ipc.send("sri.subscribe", json!({})).await;

        if let Ok(response) = ipc.send("sri.snapshot", json!({})).await
            && let Some(snapshot_value) = response.get("snapshot").cloned()
            && let Ok(snapshot) = serde_json::from_value::<SriSnapshot>(snapshot_value)
            && let Ok(mut tab_state) = state.lock()
        {
            tab_state.nodes = snapshot.nodes;
            if let Some(resources) = snapshot.latest_resources {
                tab_state.record_snapshot(resources);
            }
        }

        let connection_alive = Arc::new(AtomicBool::new(true));
        let connection_flag = Arc::clone(&connection_alive);
        let resource_state = Arc::clone(&state);
        let mut push_rx = ipc.subscribe_events();

        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                if tab_chrome::is_shutdown_event(&event) {
                    break;
                }

                if event
                    .get("stream")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    != "sri"
                {
                    continue;
                }

                let Some(event_value) = event.get("event").cloned() else {
                    continue;
                };

                let Ok(sri_event) = serde_json::from_value::<SriEvent>(event_value) else {
                    continue;
                };

                if let Ok(mut tab_state) = resource_state.lock() {
                    match sri_event {
                        SriEvent::TreeSnapshot { .. }
                        | SriEvent::TreeNavigation { .. }
                        | SriEvent::SignalReceived { .. }
                        | SriEvent::FunctionCallStub { .. } => {}
                        SriEvent::ResourceSnapshot(snapshot) => tab_state.record_snapshot(snapshot),
                        SriEvent::ResourceAlert {
                            actor,
                            resource,
                            value,
                            threshold,
                        } => tab_state.push_alert(format!(
                            "{} {} {:.1} > {:.1}",
                            actor,
                            resource_label(resource),
                            value,
                            threshold
                        )),
                        SriEvent::NodeHealthChanged { shelf_path, new, .. } => {
                            if let Some(node) = tab_state
                                .nodes
                                .iter_mut()
                                .find(|node| node.shelf_path == shelf_path)
                            {
                                node.health_status = new;
                            }
                        }
                    }
                }
            }

            connection_flag.store(false, Ordering::SeqCst);
        });

        Ok(Self {
            terminal,
            state,
            connection_alive,
            daemon_uptime_secs,
            daemon_uptime_anchor: Instant::now(),
            close_confirmation: tab_chrome::CloseConfirmation::new(),
        })
    }

    fn run_loop(&mut self) -> Result<(), CliError> {
        loop {
            if !self.connection_alive.load(Ordering::SeqCst) {
                return Ok(());
            }

            self.render()?;

            if event::poll(Duration::from_millis(100))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && let Event::Key(key) =
                    event::read().map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && key.kind == KeyEventKind::Press
            {
                match self.close_confirmation.handle_key(key.code, key.modifiers) {
                    tab_chrome::CloseAction::Confirmed => return Ok(()),
                    tab_chrome::CloseAction::Armed
                    | tab_chrome::CloseAction::Cancelled => continue,
                    tab_chrome::CloseAction::Ignored => {}
                }
            }
        }
    }

    fn cleanup(&mut self) -> Result<(), CliError> {
        tab_chrome::restore_terminal(&mut self.terminal)
    }

    fn render(&mut self) -> Result<(), CliError> {
        let state = self.state.lock().ok().map(|state| state.clone()).unwrap_or(ResourceState {
            latest_resources: None,
            nodes: Vec::new(),
            history: VecDeque::new(),
            alerts: VecDeque::new(),
        });
        let daemon_status = if self.connection_alive.load(Ordering::SeqCst) {
            "Connected"
        } else {
            "Disconnected"
        };
        let daemon_uptime_secs =
            tab_chrome::elapsed_uptime(self.daemon_uptime_secs, self.daemon_uptime_anchor);

        tab_chrome::sync_terminal_before_draw(&mut self.terminal)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        self.terminal
            .draw(|frame| {
                Self::render_frame(
                    frame,
                    &state,
                    daemon_status,
                    daemon_uptime_secs,
                    &self.close_confirmation,
                )
            })
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        Ok(())
    }

    fn render_frame(
        frame: &mut Frame,
        state: &ResourceState,
        daemon_status: &str,
        daemon_uptime_secs: u64,
        close_confirmation: &tab_chrome::CloseConfirmation,
    ) {
        if close_confirmation.is_active() {
            tab_chrome::render_close_confirmation(frame, close_confirmation.remaining_seconds());
            return;
        }

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(7), Constraint::Min(0)])
            .split(frame.area());
        let lower = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
            .split(vertical[2]);
        let lower_right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(lower[1]);

        tab_chrome::render_header(
            frame,
            vertical[0],
            "RESOURCES",
            daemon_status,
            daemon_uptime_secs,
            Some(tab_chrome::close_hint()),
        );

        frame.render_widget(
            Paragraph::new(resource_summary_lines(state))
                .block(theme::panel("Current Load"))
                .wrap(Wrap { trim: false }),
            vertical[1],
        );
        frame.render_widget(
            Paragraph::new(history_lines(&state.history))
                .block(theme::panel("Recent Samples"))
                .wrap(Wrap { trim: false }),
            lower[0],
        );
        frame.render_widget(
            Paragraph::new(health_lines(&state.nodes, &state.alerts))
                .block(theme::panel("Actor Health Summary"))
                .wrap(Wrap { trim: false }),
            lower_right[0],
        );
        frame.render_widget(
            Paragraph::new(Line::from(tab_chrome::close_hint()))
                .block(theme::panel("Status")),
            lower_right[1],
        );
    }
}

fn resource_summary_lines(state: &ResourceState) -> Vec<Line<'static>> {
    let Some(snapshot) = state.latest_resources.as_ref() else {
        return vec![Line::from("Waiting for SRI resource samples...")];
    };

    let mut lines = vec![resource_line(
        "CPU",
        snapshot.total_cpu_pct.clamp(0.0, 100.0),
        format!("{:.1}%", snapshot.total_cpu_pct),
        style_for_percent(snapshot.total_cpu_pct, 40.0, 70.0),
    )];
    lines.push(resource_line(
        "RAM",
        (snapshot.total_ram_mb.min(4096) as f32 / 4096.0) * 100.0,
        format!("{} MB process working set", snapshot.total_ram_mb),
        style_for_percent(snapshot.total_ram_mb as f32 / 32.0, 40.0, 70.0),
    ));

    if let (Some(used_mb), Some(total_mb)) = (snapshot.vram_used_mb, snapshot.vram_total_mb) {
        let percent = if total_mb == 0 {
            0.0
        } else {
            used_mb as f32 / total_mb as f32 * 100.0
        };
        lines.push(resource_line(
            "VRAM",
            percent,
            format!(
                "{:.2} GB / {:.2} GB",
                used_mb as f32 / 1024.0,
                total_mb as f32 / 1024.0
            ),
            style_for_percent(percent, 70.0, 90.0),
        ));
    } else {
        lines.push(Line::from("VRAM [----------] unavailable"));
    }

    lines
}

fn resource_line(label: &str, percent: f32, value: String, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<4}", label), theme::muted()),
        Span::styled(format!("[{}] {}", resource_bar(percent, 10), value), style),
    ])
}

fn resource_bar(percent: f32, width: usize) -> String {
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f32).round() as usize;
    format!(
        "{}{}",
        "#".repeat(filled.min(width)),
        "-".repeat(width.saturating_sub(filled.min(width)))
    )
}

fn style_for_percent(percent: f32, warn_threshold: f32, danger_threshold: f32) -> Style {
    if percent < warn_threshold {
        theme::success()
    } else if percent <= danger_threshold {
        theme::warning()
    } else {
        theme::danger()
    }
}

fn history_lines(history: &VecDeque<String>) -> Vec<Line<'static>> {
    if history.is_empty() {
        return vec![Line::from("No resource samples recorded yet.")];
    }

    history.iter().rev().cloned().map(Line::from).collect()
}

fn health_lines(nodes: &[RegisteredSriNode], alerts: &VecDeque<String>) -> Vec<Line<'static>> {
    let active = nodes
        .iter()
        .filter(|node| node.health_status == HealthStatus::Active)
        .count();
    let degraded = nodes
        .iter()
        .filter(|node| node.health_status == HealthStatus::Degraded)
        .count();
    let unavailable = nodes
        .iter()
        .filter(|node| node.health_status == HealthStatus::Unavailable)
        .count();

    let mut lines = vec![
        Line::from(format!("active: {}", active)),
        Line::from(format!("degraded: {}", degraded)),
        Line::from(format!("unavailable: {}", unavailable)),
        Line::from(""),
    ];

    if let Some(node) = nodes.iter().find(|node| node.health_status == HealthStatus::Degraded) {
        lines.push(Line::from(format!("degraded: {}", node.display_name)));
    }
    if let Some(node) = nodes.iter().find(|node| node.health_status == HealthStatus::Unavailable)
    {
        lines.push(Line::from(format!("unavailable: {}", node.display_name)));
    }

    if !alerts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("alerts:"));
        lines.extend(alerts.iter().rev().cloned().map(Line::from));
    }

    lines
}

fn format_resource_sample(snapshot: &SriResourceSnapshot) -> String {
    let vram = match (snapshot.vram_used_mb, snapshot.vram_total_mb) {
        (Some(used_mb), Some(total_mb)) if total_mb > 0 => format!(
            "vram {:.2}/{:.2}GB",
            used_mb as f32 / 1024.0,
            total_mb as f32 / 1024.0
        ),
        _ => "vram unavailable".to_string(),
    };

    format!(
        "cpu {:>5.1}% | ram {:>5} MB | {}",
        snapshot.total_cpu_pct,
        snapshot.total_ram_mb,
        vram
    )
}

fn resource_label(resource: sri::ResourceKind) -> &'static str {
    match resource {
        sri::ResourceKind::Ram => "ram",
        sri::ResourceKind::Cpu => "cpu",
        sri::ResourceKind::Vram => "vram",
    }
}