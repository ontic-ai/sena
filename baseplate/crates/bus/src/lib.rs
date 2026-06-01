use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SENA_OUTPUT_ROOT: &str = "sena_output";
pub const SENA_REPLY_TAG: &str = "sena_reply";
pub const SENA_THINK_TAG: &str = "sena_think";
pub const SENA_TASK_TAG: &str = "sena_task";
pub const SENA_CANCEL_TAG: &str = "sena_cancel";
pub const SENA_SIGNAL_TAG: &str = "sena_signal";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedModelOutput {
    pub raw: String,
    pub reply: Option<String>,
    pub think: Vec<String>,
    pub tasks: Vec<TaskRequest>,
    pub cancels: Vec<CancelRequest>,
    pub signals: Vec<SignalMessage>,
    pub unknown_tags: Vec<String>,
}

impl ParsedModelOutput {
    pub fn spoken_reply(&self) -> Option<&str> {
        self.reply.as_deref().filter(|reply| !reply.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskRequest {
    pub mode: Option<String>,
    pub capability: String,
    pub run_id: Option<String>,
    pub args: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CancelRequest {
    pub run_id: Option<String>,
    pub scope: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SignalMessage {
    pub name: Option<String>,
    pub value: String,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootStatus {
    Pending,
    Running,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootProgressEvent {
    pub actor: String,
    pub status: BootStatus,
    pub progress_percent: u8,
    pub subprocess: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationEvent {
    ModelOutputParsed(ParsedModelOutput),
    ModelOutputFallback {
        raw: String,
        reply: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Accepted,
    Running,
    Complete,
    Failed,
    CancelRequested,
    Canceling,
    Canceled,
    Stale,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub capability: String,
    pub run_id: Option<String>,
    pub status: TaskStatus,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SriIndexStatus {
    Registered,
    IndexedIn,
    IndexedOut,
    HealthChanged,
    ActiveShelfChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SriEvent {
    pub node_id: String,
    pub status: SriIndexStatus,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeechStatus {
    Idle,
    Listening,
    Speaking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechEvent {
    pub status: SpeechStatus,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEvent {
    Boot(BootProgressEvent),
    Conversation(ConversationEvent),
    Task(TaskEvent),
    Sri(SriEvent),
    Speech(SpeechEvent),
}
