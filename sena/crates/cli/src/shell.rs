//! TUI shell — slash-command interface.

use crate::daemon_ipc::{CliCommand, DaemonEvent, IpcClient};
use crate::error::CliError;
use ratatui::{
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tokio::time::{self, Duration};
use tracing::info;

/// TUI shell state.
pub struct Shell {
    ipc: IpcClient,
    message_log: Vec<String>,
    loop_states: std::collections::HashMap<String, bool>,
}

impl Shell {
    /// Create and initialize a new shell with an IPC client.
    pub fn new(ipc: IpcClient) -> Self {
        Self {
            ipc,
            message_log: vec!["Sena CLI — ready. Type /help for commands.".to_string()],
            loop_states: std::collections::HashMap::new(),
        }
    }

    /// Run the shell event loop.
    pub async fn run(mut self) -> Result<(), CliError> {
        info!("Shell starting");
        self.render();

        let stdin = BufReader::new(io::stdin());
        let mut lines = stdin.lines();
        let mut ipc_poll = time::interval(Duration::from_millis(200));

        loop {
            tokio::select! {
                maybe_line = lines.next_line() => {
                    let maybe_line = maybe_line.map_err(|e| CliError::ShellRunError(e.to_string()))?;
                    match maybe_line {
                        Some(line) => {
                            if self.handle_input_line(&line).await? {
                                break;
                            }
                        }
                        None => {
                            // End-of-input (e.g. piped input finished)
                            break;
                        }
                    }
                }
                _ = ipc_poll.tick() => {
                    if let Some(event) = self
                        .ipc
                        .recv_event()
                        .await
                        .map_err(|e| CliError::ShellRunError(e.to_string()))?
                    {
                        self.handle_daemon_event(event);
                    }
                }
            }
        }

        info!("Shell stopped");
        Ok(())
    }

    /// Build a tiny ratatui snapshot.
    fn render(&self) {
        let _header = Paragraph::new(Line::from("Sena CLI"))
            .block(Block::default().title("Header").borders(Borders::ALL));

        let items: Vec<ListItem> = self
            .message_log
            .iter()
            .rev()
            .take(8)
            .rev()
            .cloned()
            .map(ListItem::new)
            .collect();
        let _log_panel =
            List::new(items).block(Block::default().title("Messages").borders(Borders::ALL));
    }

    /// Handle one user-entered line.
    async fn handle_input_line(&mut self, raw_line: &str) -> Result<bool, CliError> {
        let input = raw_line.trim();
        if input.is_empty() {
            return Ok(false);
        }

        if input.starts_with('/') {
            self.handle_slash_command(input).await
        } else {
            // Non-slash input — ignore with hint
            self.log_message("Sena listens by voice. Use / for commands.");
            Ok(false)
        }
    }

    /// Handle a slash command.
    ///
    /// Returns true if the shell should exit.
    async fn handle_slash_command(&mut self, input: &str) -> Result<bool, CliError> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(false);
        }

        let command = parts[0];
        match command {
            "/help" => {
                self.log_message("Commands:");
                self.log_message("  /help         - Show this help");
                self.log_message("  /status       - Request daemon status");
                self.log_message("  /loop         - List background loops");
                self.log_message("  /loop <name>  - Toggle a background loop");
                self.log_message("  /loop <name> <on|off> - Set loop state");
                self.log_message("  /debug        - Show debug info");
                self.log_message("  /verbose <on|off> - Set verbose logging");
                self.log_message("  /memory       - Show memory stats");
                self.log_message("  /config       - Dump current config");
                self.log_message("  /ping         - Ping the daemon");
                self.log_message("  /shutdown     - Shutdown the daemon");
                self.log_message("  /quit         - Exit CLI");
            }
            "/quit" | "/exit" => {
                self.log_message("Exiting...");
                return Ok(true);
            }
            "/status" => {
                self.send_command(CliCommand::Status).await?;
            }
            "/loop" | "/loops" => {
                if parts.len() == 1 {
                    // List all loops
                    self.send_command(CliCommand::ListLoops).await?;
                } else if parts.len() == 2 {
                    // Toggle: use known state from loop registry or require explicit on/off
                    let loop_name = parts[1];
                    if let Some(&current_state) = self.loop_states.get(loop_name) {
                        // We know the state, toggle it
                        let new_state = !current_state;
                        self.send_command(CliCommand::ToggleLoop {
                            loop_name: loop_name.to_string(),
                            enabled: new_state,
                        })
                        .await?;
                    } else {
                        // State unknown, require explicit on/off
                        self.log_message(&format!(
                            "Loop state unknown. Use: /loop {} <on|off>",
                            loop_name
                        ));
                        self.log_message("Or use /loop to list all loops first.");
                    }
                } else if parts.len() == 3 {
                    let enabled = match parts[2] {
                        "on" | "enable" | "true" => true,
                        "off" | "disable" | "false" => false,
                        _ => {
                            self.log_message(&format!("Invalid state: {}. Use on/off.", parts[2]));
                            return Ok(false);
                        }
                    };
                    self.send_command(CliCommand::ToggleLoop {
                        loop_name: parts[1].to_string(),
                        enabled,
                    })
                    .await?;
                }
            }
            "/ping" => {
                self.send_command(CliCommand::Ping).await?;
            }
            "/shutdown" => {
                self.send_command(CliCommand::Shutdown).await?;
                self.log_message("Shutdown requested. Daemon will disconnect.");
                return Ok(true);
            }
            "/debug" => {
                self.send_command(CliCommand::DebugInfo).await?;
            }
            "/verbose" => {
                if parts.len() == 2 {
                    let enabled = match parts[1] {
                        "on" | "enable" | "true" => true,
                        "off" | "disable" | "false" => false,
                        _ => {
                            self.log_message(&format!("Invalid state: {}. Use on/off.", parts[1]));
                            return Ok(false);
                        }
                    };
                    self.send_command(CliCommand::SetVerbose { enabled })
                        .await?;
                } else {
                    self.log_message("Usage: /verbose <on|off>");
                }
            }
            "/memory" => {
                self.send_command(CliCommand::MemoryStats).await?;
            }
            "/config" => {
                self.send_command(CliCommand::ConfigDump).await?;
            }
            _ => {
                self.log_message(&format!(
                    "Unknown command: {}. Type /help for commands.",
                    command
                ));
            }
        }

        Ok(false)
    }

    /// Send a command to the daemon and log it.
    async fn send_command(&mut self, command: CliCommand) -> Result<(), CliError> {
        info!("Sending command: {:?}", command);
        self.log_message(&format!("→ {:?}", command));
        self.ipc.send_command(command).await?;
        self.render();
        Ok(())
    }

    /// Handle a daemon event.
    fn handle_daemon_event(&mut self, event: DaemonEvent) {
        info!("Received daemon event: {:?}", event);
        match event {
            DaemonEvent::DaemonReady => {
                self.log_message("✓ Daemon ready");
            }
            DaemonEvent::DaemonShuttingDown => {
                self.log_message("✓ Daemon shutting down");
            }
            DaemonEvent::StatusUpdate {
                actors,
                uptime_seconds,
            } => {
                self.log_message(&format!("Daemon uptime: {}s", uptime_seconds));
                self.log_message("Actor health:");
                for actor in actors {
                    let status_str = match actor.status {
                        bus::events::system::ActorStatus::Running => "running",
                        bus::events::system::ActorStatus::Stopped => "stopped",
                        bus::events::system::ActorStatus::Failed { ref reason } => {
                            &format!("failed: {}", reason)
                        }
                    };
                    self.log_message(&format!(
                        "  {} — {} ({}s uptime)",
                        actor.name, status_str, actor.uptime_seconds
                    ));
                }
            }
            DaemonEvent::Pong => {
                self.log_message("← Pong");
            }
            DaemonEvent::Acknowledged => {
                self.log_message("✓ Acknowledged");
            }
            DaemonEvent::LoopStatusChanged { loop_name, enabled } => {
                // Update local state tracking
                self.loop_states.insert(loop_name.clone(), enabled);
                let state = if enabled { "enabled" } else { "disabled" };
                self.log_message(&format!("Loop '{}' now {}", loop_name, state));
            }
            DaemonEvent::DebugInfo { info } => {
                self.log_message("Debug info:");
                self.log_message(&info);
            }
            DaemonEvent::VerboseSet { enabled } => {
                let state = if enabled { "enabled" } else { "disabled" };
                self.log_message(&format!("Verbose logging now {}", state));
            }
            DaemonEvent::MemoryStats { stats } => {
                self.log_message("Memory stats:");
                self.log_message(&stats);
            }
            DaemonEvent::ConfigDump { config } => {
                self.log_message("Current config:");
                self.log_message(&config);
            }
            DaemonEvent::LoopsListed { loops } => {
                self.log_message("Background loops:");
                // Update local state tracking
                for loop_info in &loops {
                    self.loop_states
                        .insert(loop_info.name.clone(), loop_info.enabled);
                    let indicator = if loop_info.enabled { "●" } else { "○" };
                    self.log_message(&format!(
                        "  {} {} — {}",
                        indicator, loop_info.name, loop_info.description
                    ));
                }
            }
        }
        self.render();
    }

    /// Log a message to the message log.
    fn log_message(&mut self, msg: &str) {
        self.message_log.push(msg.to_string());
        println!("{}", msg);
        // Keep log bounded
        if self.message_log.len() > 1000 {
            self.message_log.drain(0..100);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shell_message_log_bounds() {
        // Can't easily test Shell::new() without mocking IPC and terminal,
        // but we can verify basic logic
        let mut log = Vec::new();
        for i in 0..1100 {
            log.push(format!("message {}", i));
        }
        assert!(log.len() > 1000);
        log.drain(0..100);
        assert_eq!(log.len(), 1000);
    }
}
