//! Command handler registration.

use crate::commands::{
    config_commands::{ConfigGetHandler, ConfigSetHandler},
    events_commands::{EventsSubscribeHandler, EventsUnsubscribeHandler},
    inference_commands::{
        InferenceDiagnosticsHandler, InferenceDiagnosticsState, InferenceStatusHandler,
        ListModelsHandler, LoadModelHandler, RunInferenceHandler,
    },
    loops_commands::{LoopRegistry, LoopsListHandler, LoopsSetHandler},
    memory_commands::{MemoryQueryHandler, MemoryStatsHandler},
    runtime_commands::{
        BootWithSelectionHandler, DaemonControlMessage, OnboardingStatusHandler, PingHandler,
        RuntimeState, ShutdownHandler, StatusHandler, SubmitOnboardingConfigHandler,
        SubmitOnboardingNameHandler, TestModeRestartHandler, TestModeStatusHandler,
    },
    speech_commands::{
        SpeechListenStartHandler, SpeechListenStopHandler, SpeechSayHandler, SpeechStatusHandler,
    },
    sri_commands::{SriSnapshotHandler, SriSubscribeHandler, SriUnsubscribeHandler},
    transparency_commands::TransparencyQueryHandler,
};
use ipc::CommandRegistry;
use runtime::BootResult;
use std::sync::Arc;

pub fn register_preboot(
    registry: &mut CommandRegistry,
    state: RuntimeState,
    control_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>,
) {
    registry.register(Arc::new(PingHandler::new(state.clone())));
    registry.register(Arc::new(StatusHandler::new(state.clone(), None)));
    registry.register(Arc::new(OnboardingStatusHandler::new()));
    registry.register(Arc::new(ShutdownHandler::new(control_tx, None)));
    registry.register(Arc::new(TestModeStatusHandler::new(state.clone())));
    registry.register(Arc::new(BootWithSelectionHandler::new(state)));
}

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
    sri_state: Option<sri::SriState>,
    inference_diagnostics: InferenceDiagnosticsState,
    control_tx: tokio::sync::mpsc::UnboundedSender<DaemonControlMessage>,
) -> LoopRegistry {
    // Runtime commands
    registry.register(Arc::new(PingHandler::new(state.clone())));
    registry.register(Arc::new(StatusHandler::new(
        state.clone(),
        Some(boot_result.bus.clone()),
    )));
    registry.register(Arc::new(OnboardingStatusHandler::new()));
    registry.register(Arc::new(ShutdownHandler::new(
        control_tx.clone(),
        Some(boot_result.bus.clone()),
    )));
    registry.register(Arc::new(TestModeStatusHandler::new(state.clone())));
    registry.register(Arc::new(BootWithSelectionHandler::new(state.clone())));
    registry.register(Arc::new(TestModeRestartHandler::new(control_tx)));
    registry.register(Arc::new(SubmitOnboardingNameHandler::new(
        boot_result.bus.clone(),
    )));
    registry.register(Arc::new(SubmitOnboardingConfigHandler::new(
        boot_result.bus.clone(),
    )));

    // Inference commands
    registry.register(Arc::new(ListModelsHandler));
    registry.register(Arc::new(LoadModelHandler::new(
        boot_result.bus.clone(),
        state.clone(),
    )));
    registry.register(Arc::new(InferenceStatusHandler));
    registry.register(Arc::new(InferenceDiagnosticsHandler::new(
        inference_diagnostics,
    )));
    registry.register(Arc::new(RunInferenceHandler::new(
        boot_result.bus.clone(),
        state.clone(),
    )));

    // Speech commands
    registry.register(Arc::new(SpeechListenStartHandler::new(
        boot_result.bus.clone(),
        state.clone(),
    )));
    registry.register(Arc::new(SpeechListenStopHandler::new(
        boot_result.bus.clone(),
        state.clone(),
    )));
    registry.register(Arc::new(SpeechSayHandler::new(
        boot_result.bus.clone(),
        state.clone(),
    )));
    registry.register(Arc::new(SpeechStatusHandler));

    // Memory commands
    registry.register(Arc::new(MemoryStatsHandler::new(
        boot_result.bus.clone(),
        state.clone(),
    )));
    registry.register(Arc::new(MemoryQueryHandler::new(
        boot_result.bus.clone(),
        state.clone(),
    )));

    // Config commands
    registry.register(Arc::new(ConfigGetHandler));
    registry.register(Arc::new(ConfigSetHandler::new(
        boot_result.bus.clone(),
        boot_result.conversation_config.clone(),
    )));

    // Event commands
    registry.register(Arc::new(EventsSubscribeHandler));
    registry.register(Arc::new(EventsUnsubscribeHandler));
    registry.register(Arc::new(SriSubscribeHandler::new(state.clone())));
    registry.register(Arc::new(SriUnsubscribeHandler::new(state.clone())));
    registry.register(Arc::new(SriSnapshotHandler::new(state.clone(), sri_state)));

    // Loop control commands
    let loop_registry = LoopRegistry::new();
    registry.register(Arc::new(LoopsListHandler::new(loop_registry.clone())));
    registry.register(Arc::new(LoopsSetHandler::new(
        loop_registry.clone(),
        boot_result.bus.clone(),
    )));

    // Transparency commands
    registry.register(Arc::new(TransparencyQueryHandler::new(
        boot_result.bus.clone(),
        state,
    )));

    loop_registry
}
