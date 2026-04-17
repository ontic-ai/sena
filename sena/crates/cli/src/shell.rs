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
}

impl Shell {
    /// Create and initialize a new shell.
    pub async fn new() -> Result<Self, CliError> {
        let ipc = IpcClient::connect().await?;

        Ok(Self {
            ipc,
            message_log: vec!["Sena CLI — ready. Type /help for commands.".to_string()],
        })
    }

    /// Run the shell event loop.
    pub async fn run(mut self) -> Result<(), CliError> {
        info!("Shell starting");
        self.render_stub();

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

    /// Build a tiny ratatui snapshot to keep the shell as a TUI-oriented stub.
    fn render_stub(&self) {
        let _header = Paragraph::new(Line::from("Sena CLI (IPC stub)"))
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
                self.log_message("  /help      - Show this help");
                self.log_message("  /status    - Request daemon status");
                self.log_message("  /loops     - List background loops");
                self.log_message("  /loops <name> - Toggle a background loop");
                self.log_message("  /ping      - Ping the daemon");
                self.log_message("  /shutdown  - Shutdown the daemon");
                self.log_message("  /quit      - Exit CLI");
            }
            "/quit" | "/exit" => {
                self.log_message("Exiting...");
                return Ok(true);
            }
            "/status" => {
                self.send_command(CliCommand::Status).await?;
            }
            "/loops" => {
                if parts.len() == 1 {
                    self.send_command(CliCommand::ListLoops).await?;
                } else {
                    self.send_command(CliCommand::ToggleLoop {
                        loop_name: parts[1].to_string(),
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
        self.render_stub();
        Ok(())
    }

    /// Handle a daemon event.
    fn handle_daemon_event(&mut self, event: DaemonEvent) {
        info!("Received daemon event: {:?}", event);
        self.log_message(&format!("← {:?}", event));
        self.render_stub();
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
