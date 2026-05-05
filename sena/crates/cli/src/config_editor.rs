use crate::error::CliError;
use crate::theme;
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
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, Wrap},
};
use serde_json::{Value, json};
use std::io;
use tracing::info;

#[derive(Clone)]
enum ConfigFieldKind {
    Text,
    Bool,
}

#[derive(Clone)]
struct ConfigField {
    path: String,
    value: String,
    kind: ConfigFieldKind,
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
    exit_armed: bool,
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
            exit_armed: false,
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
            kind: ConfigFieldKind::Text,
            editable: false,
            dirty: false,
            save_error: None,
        });
        fields.push(ConfigField {
            path: "crypto.key_version".to_string(),
            value: "managed by runtime".to_string(),
            kind: ConfigFieldKind::Text,
            editable: false,
            dirty: false,
            save_error: None,
        });
        fields.push(ConfigField {
            path: "bus.schema_version".to_string(),
            value: "managed by runtime".to_string(),
            kind: ConfigFieldKind::Text,
            editable: false,
            dirty: false,
            save_error: None,
        });

        self.fields = fields;
        self.status_line =
            "Loaded config. Enter edits, S to save, Esc to discard and return.".to_string();
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

        let kind = match value {
            Some(Value::Bool(_)) => ConfigFieldKind::Bool,
            _ => ConfigFieldKind::Text,
        };

        fields.push(ConfigField {
            path: path.to_string(),
            value: value_str,
            kind,
            editable,
            dirty: false,
            save_error: None,
        });
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<(), CliError> {
        if !matches!(code, KeyCode::Esc) {
            self.exit_armed = false;
        }

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
                        if matches!(field.kind, ConfigFieldKind::Bool) {
                            self.status_line = "Use ← / → to toggle true or false".to_string();
                        } else {
                            self.edit_mode = true;
                            self.edit_buffer = field.value.clone();
                            self.status_line = format!("Editing {}", field.path);
                        }
                    } else {
                        self.status_line = "managed by runtime".to_string();
                    }
                }
            }
            KeyCode::Left | KeyCode::Right => {
                if let Some(field) = self.fields.get_mut(self.selected)
                    && field.editable
                    && matches!(field.kind, ConfigFieldKind::Bool)
                {
                    field.value = if field.value == "true" {
                        "false".to_string()
                    } else {
                        "true".to_string()
                    };
                    field.dirty = true;
                    field.save_error = None;
                    self.status_line =
                        format!("{} set to {} (pending save)", field.path, field.value);
                }
            }
            KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_changes().await?;
            }
            KeyCode::Char('s') => {
                self.save_changes().await?;
            }
            KeyCode::Esc => {
                if self.exit_armed {
                    self.should_exit = true;
                } else {
                    self.exit_armed = true;
                    self.status_line = "Press Esc again to exit the config editor.".to_string();
                }
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
                        Constraint::Min(8),
                        Constraint::Length(2),
                    ])
                    .split(frame.area());

                let body_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
                    .split(chunks[1]);

                let header = Paragraph::new(Line::from(vec![
                    Span::styled("SENA CONFIG", theme::title_style()),
                    Span::styled("  [S] Save", theme::text()),
                    Span::styled("  [Esc] Back", theme::muted()),
                ]))
                .block(theme::panel("Config Editor"));
                frame.render_widget(header, chunks[0]);

                let items: Vec<ListItem> = self
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(idx, field)| {
                        let style = if idx == self.selected {
                            theme::selected()
                        } else if field.editable {
                            theme::text()
                        } else {
                            theme::readonly()
                        };

                        let mut parts = vec![Span::styled(field.path.clone(), style)];
                        parts.push(Span::styled(" = ", theme::muted()));
                        parts.push(Span::styled(field.value.clone(), style));
                        if field.dirty {
                            parts.push(Span::styled("  [modified]", theme::warning()));
                        }
                        if !field.editable {
                            parts.push(Span::styled("  [runtime managed]", theme::muted()));
                        }
                        if field.save_error.is_some() {
                            parts.push(Span::styled("  [save failed]", theme::danger()));
                        }

                        let line = Line::from(parts);
                        ListItem::new(line)
                    })
                    .collect();

                let list = List::new(items).block(theme::panel(if self.edit_mode {
                    "Fields (editing: Enter commit, Esc cancel)"
                } else {
                    "Fields (Up/Down/Tab navigate, Enter edit)"
                }));
                frame.render_widget(list, body_chunks[0]);

                if let Some(field) = self.fields.get(self.selected) {
                    let editing_value = if self.edit_mode {
                        self.edit_buffer.as_str()
                    } else {
                        field.value.as_str()
                    };
                    let mut detail_lines = vec![
                        Line::from(vec![
                            Span::styled("Field ", theme::muted()),
                            Span::styled(field.path.as_str(), theme::title_style()),
                        ]),
                        Line::from(vec![
                            Span::styled("Access ", theme::muted()),
                            Span::styled(
                                if field.editable {
                                    "editable"
                                } else {
                                    "runtime managed"
                                },
                                if field.editable {
                                    theme::success()
                                } else {
                                    theme::warning()
                                },
                            ),
                        ]),
                        Line::from(vec![
                            Span::styled("State ", theme::muted()),
                            Span::styled(
                                if self.edit_mode {
                                    "editing buffer"
                                } else if field.dirty {
                                    "modified, not saved"
                                } else {
                                    "saved"
                                },
                                if self.edit_mode || field.dirty {
                                    theme::warning()
                                } else {
                                    theme::success()
                                },
                            ),
                        ]),
                        Line::from(""),
                        Line::from(vec![Span::styled("Value", theme::muted())]),
                        Line::from(editing_value),
                    ];

                    if let Some(err) = &field.save_error {
                        detail_lines.push(Line::from(""));
                        detail_lines.push(Line::from(vec![
                            Span::styled("Last save error ", theme::muted()),
                            Span::styled(err.as_str(), theme::danger()),
                        ]));
                    }

                    detail_lines.push(Line::from(""));
                    detail_lines.push(Line::from(vec![Span::styled("Workflow", theme::muted())]));
                    if matches!(field.kind, ConfigFieldKind::Bool) && field.editable {
                        detail_lines
                            .push(Line::from("Use ← or → to switch between true and false."));
                    } else {
                        detail_lines.push(Line::from("Enter edits the selected field."));
                    }
                    detail_lines.push(Line::from("S saves all modified editable fields."));
                    detail_lines.push(Line::from("Esc cancels editing or exits the editor."));

                    let details = Paragraph::new(detail_lines)
                        .block(theme::focused_panel("Selection"))
                        .style(theme::text())
                        .wrap(Wrap { trim: false });
                    frame.render_widget(details, body_chunks[1]);
                }

                let footer = Paragraph::new(self.status_line.as_str())
                    .style(theme::warning())
                    .block(theme::focused_panel("Status"));
                frame.render_widget(footer, chunks[2]);
            })
            .map_err(|e| CliError::TuiRenderError(e.to_string()))?;

        Ok(())
    }
}
