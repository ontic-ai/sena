use crate::error::CliError;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ipc::IpcClient;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use serde_json::{Value, json};
use std::io;
use tracing::info;

#[derive(Clone)]
struct ConfigField {
    path: String,
    value: String,
    editable: bool,
    dirty: bool,
    save_error: Option<String>,
}

pub struct ConfigEditor<'a> {
    ipc: &'a mut IpcClient,
    fields: Vec<ConfigField>,
    selected: usize,
    edit_mode: bool,
    edit_buffer: String,
    status_line: String,
    should_exit: bool,
}

impl<'a> ConfigEditor<'a> {
    pub fn new(ipc: &'a mut IpcClient) -> Self {
        Self {
            ipc,
            fields: Vec::new(),
            selected: 0,
            edit_mode: false,
            edit_buffer: String::new(),
            status_line: "Loading config...".to_string(),
            should_exit: false,
        }
    }

    pub async fn run(&mut self) -> Result<(), CliError> {
        info!("Config editor starting");

        self.load_fields().await?;

        enable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend).map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        while !self.should_exit {
            self.render(&mut terminal)?;

            if event::poll(std::time::Duration::from_millis(100))
                .map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && let Event::Key(key) =
                    event::read().map_err(|e| CliError::TuiRenderError(e.to_string()))?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key.code, key.modifiers).await?;
            }
        }

        disable_raw_mode().map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;
        terminal
            .show_cursor()
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        info!("Config editor stopped");
        Ok(())
    }

    async fn load_fields(&mut self) -> Result<(), CliError> {
        let response = self
            .ipc
            .send("config.get", json!({}))
            .await
            .map_err(|e| CliError::IpcSendError(e.to_string()))?;

        let mut fields = Vec::new();

        self.push_field_from_json(
            &mut fields,
            "file_watch_paths",
            response.get("file_watch_paths"),
            true,
        );
        self.push_field_from_json(
            &mut fields,
            "clipboard_observation_enabled",
            response.get("clipboard_observation_enabled"),
            true,
        );
        self.push_field_from_json(
            &mut fields,
            "speech_enabled",
            response.get("speech_enabled"),
            true,
        );
        self.push_field_from_json(
            &mut fields,
            "inference_max_tokens",
            response.get("inference_max_tokens"),
            true,
        );
        self.push_field_from_json(
            &mut fields,
            "auto_tune_tokens",
            response.get("auto_tune_tokens"),
            true,
        );
        self.push_field_from_json(
            &mut fields,
            "auto_tune_min_tokens",
            response.get("auto_tune_min_tokens"),
            true,
        );
        self.push_field_from_json(
            &mut fields,
            "auto_tune_max_tokens",
            response.get("auto_tune_max_tokens"),
            true,
        );

        // Add explicit non-editable fields required by process_split UX contract.
        fields.push(ConfigField {
            path: "models_dir".to_string(),
            value: "managed by runtime".to_string(),
            editable: false,
            dirty: false,
            save_error: None,
        });
        fields.push(ConfigField {
            path: "crypto.key_version".to_string(),
            value: "managed by runtime".to_string(),
            editable: false,
            dirty: false,
            save_error: None,
        });
        fields.push(ConfigField {
            path: "bus.schema_version".to_string(),
            value: "managed by runtime".to_string(),
            editable: false,
            dirty: false,
            save_error: None,
        });

        self.fields = fields;
        self.status_line = "Loaded config. Enter edits, S to save, Esc to discard and return.".to_string();
        Ok(())
    }

    fn push_field_from_json(
        &self,
        fields: &mut Vec<ConfigField>,
        path: &str,
        value: Option<&Value>,
        editable: bool,
    ) {
        let value_str = match value {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Bool(b)) => b.to_string(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(";"),
            Some(v) => v.to_string(),
            None => "".to_string(),
        };

        fields.push(ConfigField {
            path: path.to_string(),
            value: value_str,
            editable,
            dirty: false,
            save_error: None,
        });
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<(), CliError> {
        if self.edit_mode {
            match code {
                KeyCode::Esc => {
                    self.edit_mode = false;
                    self.edit_buffer.clear();
                    self.status_line = "Edit canceled".to_string();
                }
                KeyCode::Enter => {
                    if let Some(field) = self.fields.get_mut(self.selected) {
                        field.value = self.edit_buffer.clone();
                        field.dirty = true;
                        field.save_error = None;
                    }
                    self.edit_mode = false;
                    self.edit_buffer.clear();
                    self.status_line = "Field updated (pending save)".to_string();
                }
                KeyCode::Backspace => {
                    self.edit_buffer.pop();
                }
                KeyCode::Char(c) => {
                    self.edit_buffer.push(c);
                }
                _ => {}
            }
            return Ok(());
        }

        match code {
            KeyCode::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                if self.selected + 1 < self.fields.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(field) = self.fields.get(self.selected) {
                    if field.editable {
                        self.edit_mode = true;
                        self.edit_buffer = field.value.clone();
                        self.status_line = format!("Editing {}", field.path);
                    } else {
                        self.status_line = "managed by runtime".to_string();
                    }
                }
            }
            KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_changes().await?;
            }
            KeyCode::Char('s') => {
                self.save_changes().await?;
            }
            KeyCode::Esc => {
                self.should_exit = true;
            }
            _ => {}
        }

        Ok(())
    }

    async fn save_changes(&mut self) -> Result<(), CliError> {
        let mut any_saved = false;
        let mut any_failed = false;

        for idx in 0..self.fields.len() {
            let should_save = self.fields[idx].dirty && self.fields[idx].editable;
            if !should_save {
                continue;
            }

            let path = self.fields[idx].path.clone();
            let value = self.fields[idx].value.clone();

            match self
                .ipc
                .send("config.set", json!({"path": path, "value": value}))
                .await
            {
                Ok(_) => {
                    self.fields[idx].dirty = false;
                    self.fields[idx].save_error = None;
                    any_saved = true;
                }
                Err(e) => {
                    self.fields[idx].save_error = Some(e.to_string());
                    any_failed = true;
                }
            }
        }

        self.status_line = if any_failed {
            "Some fields failed to save (see inline errors)".to_string()
        } else if any_saved {
            "All modified fields saved".to_string()
        } else {
            "No changes to save".to_string()
        };

        Ok(())
    }

    fn render(
        &self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<(), CliError> {
        terminal
            .draw(|frame| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(0),
                        Constraint::Length(2),
                    ])
                    .split(frame.area());

                let header = Paragraph::new("SENA CONFIG    [S] Save   [Esc] Discard and Back")
                    .block(Block::default().borders(Borders::ALL).title("Config Editor"));
                frame.render_widget(header, chunks[0]);

                let items: Vec<ListItem> = self
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(idx, field)| {
                        let mut style = if field.editable {
                            Style::default().fg(Color::White)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };

                        if idx == self.selected {
                            style = style.add_modifier(Modifier::BOLD).bg(Color::DarkGray);
                        }

                        let dirty = if field.dirty { " *" } else { "" };
                        let err = field
                            .save_error
                            .as_ref()
                            .map(|e| format!("  ! {}", e))
                            .unwrap_or_default();

                        let line = Line::from(vec![
                            Span::styled(format!("{}{}", field.path, dirty), style),
                            Span::raw(" = "),
                            Span::styled(field.value.clone(), style),
                            Span::styled(err, Style::default().fg(Color::Red)),
                        ]);
                        ListItem::new(line)
                    })
                    .collect();

                let list = List::new(items).block(Block::default().borders(Borders::ALL).title(
                    if self.edit_mode {
                        "Fields (editing: Enter commit, Esc cancel)"
                    } else {
                        "Fields (Up/Down/Tab navigate, Enter edit)"
                    },
                ));
                frame.render_widget(list, chunks[1]);

                let footer = Paragraph::new(self.status_line.as_str())
                    .style(Style::default().fg(Color::Yellow))
                    .block(Block::default().borders(Borders::ALL).title("Status"));
                frame.render_widget(footer, chunks[2]);
            })
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        Ok(())
    }
}
