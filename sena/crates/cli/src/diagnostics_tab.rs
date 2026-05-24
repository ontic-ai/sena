use crate::error::CliError;
use crate::tab_chrome;
use crate::theme;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ipc::IpcClient;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    text::Line,
    widgets::{Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Deserialize)]
struct DiagnosticsSnapshot {
    prompt: String,
    source: String,
    full_text: String,
    generated_token_count: usize,
    stop_condition: String,
    raw_generated_text: String,
    max_tokens: usize,
    temperature: f32,
    repeat_penalty: f32,
    top_k: u32,
    top_p: f32,
    #[serde(default)]
    stop_sequences: Vec<String>,
    causal_id: u64,
}

struct DiagnosticsTab {
    terminal: tab_chrome::AppTerminal,
    snapshot: Arc<Mutex<Option<DiagnosticsSnapshot>>>,
    connection_alive: Arc<AtomicBool>,
    daemon_uptime_secs: u64,
    daemon_uptime_anchor: Instant,
    close_armed_at: Option<Instant>,
}

pub async fn run(mut ipc: IpcClient) -> Result<(), CliError> {
    let mut tab = DiagnosticsTab::new(&mut ipc).await?;
    let result = tab.run_loop();
    let cleanup_result = tab.cleanup();
    result.and(cleanup_result)
}

impl DiagnosticsTab {
    async fn new(ipc: &mut IpcClient) -> Result<Self, CliError> {
        let terminal = tab_chrome::init_terminal()?;
        let daemon_uptime_secs = ipc
            .send("runtime.ping", json!({}))
            .await
            .ok()
            .and_then(|response| response.get("uptime_seconds").and_then(|value| value.as_u64()))
            .unwrap_or(0);
        let snapshot = Arc::new(Mutex::new(None));

        if let Ok(response) = ipc.send("inference.diagnostics", json!({})).await
            && let Some(snapshot_value) = response.get("snapshot").cloned()
            && !snapshot_value.is_null()
            && let Ok(latest) = serde_json::from_value::<DiagnosticsSnapshot>(snapshot_value)
            && let Ok(mut state) = snapshot.lock()
        {
            *state = Some(latest);
        }

        let connection_alive = Arc::new(AtomicBool::new(true));
        let connection_flag = Arc::clone(&connection_alive);
        let snapshot_state = Arc::clone(&snapshot);
        let mut push_rx = ipc.subscribe_events();

        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                if event
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    == "InferenceDiagnosticsUpdated"
                    && let Some(data) = event.get("data").cloned()
                    && let Ok(latest) = serde_json::from_value::<DiagnosticsSnapshot>(data)
                    && let Ok(mut state) = snapshot_state.lock()
                {
                    *state = Some(latest);
                }
            }

            connection_flag.store(false, Ordering::SeqCst);
        });

        Ok(Self {
            terminal,
            snapshot,
            connection_alive,
            daemon_uptime_secs,
            daemon_uptime_anchor: Instant::now(),
            close_armed_at: None,
        })
    }

    fn run_loop(&mut self) -> Result<(), CliError> {
        loop {
            self.render()?;

            if event::poll(Duration::from_millis(100))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && let Event::Key(key) =
                    event::read().map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && key.kind == KeyEventKind::Press
            {
                if tab_chrome::handle_double_ctrl_x(
                    key.code,
                    key.modifiers,
                    &mut self.close_armed_at,
                ) {
                    return Ok(());
                }
            }
        }
    }

    fn cleanup(&mut self) -> Result<(), CliError> {
        tab_chrome::restore_terminal(&mut self.terminal)
    }

    fn render(&mut self) -> Result<(), CliError> {
        let snapshot = self.snapshot.lock().ok().and_then(|state| state.clone());
        let daemon_status = if self.connection_alive.load(Ordering::SeqCst) {
            "Connected"
        } else {
            "Disconnected"
        };
        let daemon_uptime_secs =
            tab_chrome::elapsed_uptime(self.daemon_uptime_secs, self.daemon_uptime_anchor);

        self.terminal
            .draw(|frame| {
                Self::render_frame(
                    frame,
                    snapshot.as_ref(),
                    daemon_status,
                    daemon_uptime_secs,
                    self.close_armed_at,
                )
            })
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        Ok(())
    }

    fn render_frame(
        frame: &mut Frame,
        snapshot: Option<&DiagnosticsSnapshot>,
        daemon_status: &str,
        daemon_uptime_secs: u64,
        close_armed_at: Option<Instant>,
    ) {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(2)])
            .split(frame.area());
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(vertical[1]);

        tab_chrome::render_header(
            frame,
            vertical[0],
            "DIAGNOSTICS",
            daemon_status,
            daemon_uptime_secs,
            Some(tab_chrome::close_hint(close_armed_at)),
        );

        let prompt_text = snapshot
            .map(|snapshot| {
                format!(
                    "source: {}\ncausal_id: {}\n\n{}",
                    snapshot.source, snapshot.causal_id, snapshot.prompt
                )
            })
            .unwrap_or_else(|| "No completed inference is available yet.".to_string());
        let trace_text = snapshot
            .map(|snapshot| {
                let stop_sequences = if snapshot.stop_sequences.is_empty() {
                    "(none)".to_string()
                } else {
                    snapshot.stop_sequences.join(", ")
                };

                format!(
                    "generated_tokens: {}\nstop_condition: {}\nmax_tokens: {}\ntemperature: {:.2}\ntop_k: {}\ntop_p: {:.2}\nrepeat_penalty: {:.2}\nstop_sequences: {}\n\nraw_generated_text:\n{}\n\nfinal_output:\n{}",
                    snapshot.generated_token_count,
                    snapshot.stop_condition,
                    snapshot.max_tokens,
                    snapshot.temperature,
                    snapshot.top_k,
                    snapshot.top_p,
                    snapshot.repeat_penalty,
                    stop_sequences,
                    snapshot.raw_generated_text,
                    snapshot.full_text,
                )
            })
            .unwrap_or_else(|| "Waiting for inference diagnostics updates...".to_string());

        frame.render_widget(
            Paragraph::new(prompt_text)
                .block(theme::panel("Prompt Sent To Model"))
                .wrap(Wrap { trim: false }),
            columns[0],
        );
        frame.render_widget(
            Paragraph::new(trace_text)
                .block(theme::panel("Generation Trace"))
                .wrap(Wrap { trim: false }),
            columns[1],
        );
        frame.render_widget(
            Paragraph::new(Line::from(tab_chrome::close_hint(close_armed_at)))
                .block(theme::panel("Status")),
            vertical[2],
        );
    }
}