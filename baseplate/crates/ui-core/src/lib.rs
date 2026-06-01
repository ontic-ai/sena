use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoaderStatus {
    Pending,
    Running,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderSubprocessSnapshot {
    pub name: String,
    pub status: LoaderStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderActorSnapshot {
    pub name: String,
    pub status: LoaderStatus,
    pub progress_percent: u8,
    pub subprocesses: Vec<LoaderSubprocessSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapSnapshot {
    pub title: String,
    pub active_actor: Option<String>,
    pub actors: Vec<LoaderActorSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeakerRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationLine {
    pub role: SpeakerRole,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalKind {
    SttPartial,
    SttFinal,
    Think,
    Task,
    Cancel,
    Sri,
    Download,
    Fault,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalLine {
    pub kind: SignalKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CliSnapshot {
    pub conversation: Vec<ConversationLine>,
    pub signals: Vec<SignalLine>,
}
