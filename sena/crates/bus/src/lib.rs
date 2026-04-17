//! Event bus, actor trait, typed events.

pub mod actor;
pub mod bus;
pub mod causal;
pub mod events;

pub use actor::{Actor, ActorError};
pub use bus::{BusError, Event, EventBus};
pub use causal::CausalId;
pub use events::system::ModelKind;
pub use events::{
    CTPEvent, ContextSnapshot, InferenceEvent, InferenceFailureOrigin, InferenceSource,
    MemoryEvent, ModelEvent, PlatformEvent, Priority, SoulEvent, SoulSummary, SpeechEvent,
    SystemEvent, TelemetryEvent, TranscribedWord,
};
