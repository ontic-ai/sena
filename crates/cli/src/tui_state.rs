//! TUI state management — session stats, conversation log, editor state.

use std::time::Instant;

/// Session statistics displayed in the TUI.
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub start_time: Instant,
    pub messages_sent: usize,
    pub tokens_received: usize,
}

impl SessionStats {
    /// Create a new session stats tracker.
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            messages_sent: 0,
            tokens_received: 0,
        }
    }

    /// Format elapsed time as "Xm Ys".
    pub fn elapsed_formatted(&self) -> String {
        let elapsed = self.start_time.elapsed().as_secs();
        let minutes = elapsed / 60;
        let seconds = elapsed % 60;
        if minutes > 0 {
            format!("{}m {}s", minutes, seconds)
        } else {
            format!("{}s", seconds)
        }
    }
}

impl Default for SessionStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Role of a message in the conversation log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    /// User input message.
    User,
    /// Sena AI response.
    Sena,
    /// System notification or status.
    System,
    /// Warning message.
    Warning,
}

/// A single message in the conversation log.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub text: String,
}

impl Message {
    /// Create a new message.
    pub fn new(role: MessageRole, text: String) -> Self {
        Self { role, text }
    }
}

/// State for the inline line editor with history.
#[derive(Debug, Clone)]
pub struct EditorState {
    /// Current input buffer.
    pub input: String,
    /// Command history.
    pub history: Vec<String>,
    /// Current position in history (None = not navigating history).
    pub history_index: Option<usize>,
    /// Temporary buffer for current input when navigating history.
    temp_buffer: String,
}

impl EditorState {
    /// Create a new editor state.
    pub fn new() -> Self {
        Self {
            input: String::new(),
            history: Vec::new(),
            history_index: None,
            temp_buffer: String::new(),
        }
    }

    /// Navigate to the previous command in history.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            // Save current input and jump to most recent.
            self.temp_buffer = self.input.clone();
            self.history_index = Some(self.history.len() - 1);
        } else if let Some(idx) = self.history_index {
            // Move to older entry if possible.
            if idx > 0 {
                self.history_index = Some(idx - 1);
            }
        }
        if let Some(idx) = self.history_index {
            self.input = self.history[idx].clone();
        }
    }

    /// Navigate to the next command in history.
    pub fn history_next(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                self.history_index = Some(idx + 1);
                self.input = self.history[idx + 1].clone();
            } else {
                // Restore temp buffer (return to current input).
                self.history_index = None;
                self.input = self.temp_buffer.clone();
            }
        }
    }

    /// Add a command to history.
    pub fn push_history(&mut self, cmd: &str) {
        if !cmd.is_empty()
            && (self.history.is_empty() || self.history.last() != Some(&cmd.to_string()))
        {
            self.history.push(cmd.to_string());
        }
        self.history_index = None;
        self.temp_buffer.clear();
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_stats_new_creates_with_zero_counts() {
        let stats = SessionStats::new();
        assert_eq!(stats.messages_sent, 0);
        assert_eq!(stats.tokens_received, 0);
    }

    #[test]
    fn session_stats_elapsed_formatted_shows_seconds() {
        let stats = SessionStats::new();
        let formatted = stats.elapsed_formatted();
        assert!(formatted.ends_with('s'));
    }

    #[test]
    fn message_new_constructs_correctly() {
        let msg = Message::new(MessageRole::User, "hello".to_string());
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.text, "hello");
    }

    #[test]
    fn editor_state_new_creates_empty() {
        let editor = EditorState::new();
        assert!(editor.input.is_empty());
        assert!(editor.history.is_empty());
        assert!(editor.history_index.is_none());
    }

    #[test]
    fn editor_state_push_history_adds_command() {
        let mut editor = EditorState::new();
        editor.push_history("test");
        assert_eq!(editor.history.len(), 1);
        assert_eq!(editor.history[0], "test");
    }

    #[test]
    fn editor_state_push_history_ignores_duplicates() {
        let mut editor = EditorState::new();
        editor.push_history("same");
        editor.push_history("same");
        assert_eq!(editor.history.len(), 1);
    }

    #[test]
    fn editor_state_push_history_ignores_empty() {
        let mut editor = EditorState::new();
        editor.push_history("");
        assert!(editor.history.is_empty());
    }

    #[test]
    fn editor_state_history_prev_navigates_backward() {
        let mut editor = EditorState::new();
        editor.push_history("first");
        editor.push_history("second");
        editor.push_history("third");

        editor.input = "current".to_string();
        editor.history_prev();
        assert_eq!(editor.input, "third");
        assert_eq!(editor.temp_buffer, "current");

        editor.history_prev();
        assert_eq!(editor.input, "second");

        editor.history_prev();
        assert_eq!(editor.input, "first");

        // Should stay at first
        editor.history_prev();
        assert_eq!(editor.input, "first");
    }

    #[test]
    fn editor_state_history_next_navigates_forward() {
        let mut editor = EditorState::new();
        editor.push_history("first");
        editor.push_history("second");

        editor.input = "current".to_string();
        editor.history_prev(); // -> "second"
        editor.history_prev(); // -> "first"

        editor.history_next(); // -> "second"
        assert_eq!(editor.input, "second");

        editor.history_next(); // -> "current" (temp buffer)
        assert_eq!(editor.input, "current");
    }

    #[test]
    fn editor_state_history_empty_returns_early() {
        let mut editor = EditorState::new();
        editor.input = "test".to_string();
        editor.history_prev();
        assert_eq!(editor.input, "test"); // unchanged
    }
}
