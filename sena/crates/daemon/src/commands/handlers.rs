//! Command handler registration.

use crate::commands::{
    config_commands::{ConfigGetHandler, ConfigSetHandler},
    events_commands::{EventsSubscribeHandler, EventsUnsubscribeHandler},
    inference_commands::{
        InferenceStatusHandler, ListModelsHandler, LoadModelHandler, RunInferenceHandler,
    },
    loops_commands::{LoopRegistry, LoopsListHandler, LoopsSetHandler},
    memory_commands::{MemoryQueryHandler, MemoryStatsHandler},
    runtime_commands::{
        PingHandler, RuntimeState, ShutdownHandler, StatusHandler, SubmitOnboardingConfigHandler,
        SubmitOnboardingNameHandler,
    },
    speech_commands::{SpeechListenStartHandler, SpeechListenStopHandler, SpeechStatusHandler},
};
use ipc::CommandRegistry;
use runtime::BootResult;
use std::sync::Arc;

/// Register all daemon command handlers with the IPC command registry.
///
/// This function is called during daemon boot, after `runtime::boot()` completes.
///
/// # Arguments
///
/// * `registry` - Mutable reference to the command registry
/// * `boot_result` - Boot result containing bus and actor handles (for future use)
/// * `state` - Runtime state shared across command handlers
/// * `shutdown_tx` - Channel sender for triggering graceful shutdown
///
/// # Returns
///
/// Returns the loop registry used by loop control handlers for tracking state.
pub fn register_all(
    registry: &mut CommandRegistry,
    boot_result: &BootResult,
    state: RuntimeState,
    shutdown_tx: tokio::sync::mpsc::UnboundedSender<()>,
) -> LoopRegistry {
    // Runtime commands
    registry.register(Arc::new(PingHandler::new(state.clone())));
    registry.register(Arc::new(StatusHandler::new(
        state.clone(),
        boot_result.bus.clone(),
    )));
    registry.register(Arc::new(ShutdownHandler::new(
        shutdown_tx,
        boot_result.bus.clone(),
    )));
    registry.register(Arc::new(SubmitOnboardingNameHandler::new(
        boot_result.bus.clone(),
    )));
    registry.register(Arc::new(SubmitOnboardingConfigHandler::new(
        boot_result.bus.clone(),
    )));

    // Inference commands
    registry.register(Arc::new(ListModelsHandler));
    registry.register(Arc::new(LoadModelHandler));
    registry.register(Arc::new(InferenceStatusHandler));
    registry.register(Arc::new(RunInferenceHandler));

    // Speech commands
    registry.register(Arc::new(SpeechListenStartHandler));
    registry.register(Arc::new(SpeechListenStopHandler));
    registry.register(Arc::new(SpeechStatusHandler));

    // Memory commands
    registry.register(Arc::new(MemoryStatsHandler));
    registry.register(Arc::new(MemoryQueryHandler));

    // Config commands
    registry.register(Arc::new(ConfigGetHandler));
    registry.register(Arc::new(ConfigSetHandler));

    // Event commands
    registry.register(Arc::new(EventsSubscribeHandler));
    registry.register(Arc::new(EventsUnsubscribeHandler));

    // Loop control commands
    let loop_registry = LoopRegistry::new();
    registry.register(Arc::new(LoopsListHandler::new(loop_registry.clone())));
    registry.register(Arc::new(LoopsSetHandler::new(
        loop_registry.clone(),
        boot_result.bus.clone(),
    )));

    loop_registry
}
