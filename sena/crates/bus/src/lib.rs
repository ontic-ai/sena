//! Event bus, actor trait, typed events.

pub mod actor;
pub mod bus;
pub mod causal;
pub mod events;

pub use actor::{Actor, ActorError};
pub use bus::{BusError, Event, EventBus};
pub use causal::CausalId;
pub use events::soul::SoulSummary;
pub use events::system::ModelKind;
pub use events::{
    CTPEvent, ContextInterpretationInput, ContextSnapshot, DownloadEvent, InferenceEvent,
    InferenceFailureOrigin, InferenceSource, MemoryEvent, ModelEvent, PlatformEvent, Priority,
    SoulEvent, SpeechEvent, SystemEvent, TelemetryEvent, TranscribedWord, TransparencyEvent,
    TransparencyQuery, TransparencyResult,
};
