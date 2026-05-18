use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::builtin_nodes::scaffold_builtin_nodes;
use crate::{
    FunctionStubStatus, HealthStatus, ResourceKind, SignalSource, SriEvent, SriRegistry,
    SriResourceSnapshot, SriSnapshot, TreeAction,
};
use bus::{
    CTPEvent, Event, EventBus, InferenceEvent, MemoryEvent, SoulEvent, SpeechEvent, SystemEvent,
};
use chrono::Utc;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid};
use tokio::sync::broadcast;

const AUTO_CLOSE_AFTER: Duration = Duration::from_secs(10);
const RESOURCE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const ALERT_COOLDOWN: Duration = Duration::from_secs(10);
const SIGNAL_DEDUP_WINDOW: Duration = Duration::from_secs(5);
const UNAVAILABLE_STUB_DEDUP_WINDOW: Duration = Duration::from_secs(60);
const VRAM_STALE_AFTER: Duration = Duration::from_secs(5);
const MAX_SIGNAL_SUMMARY_CHARS: usize = 80;

#[derive(Clone)]
pub struct SriState {
    registry: SriRegistry,
    runtime: Arc<RwLock<SriRuntimeState>>,
}

#[derive(Default)]
struct SriRuntimeState {
    open_shelves: HashSet<String>,
    last_activity: HashMap<String, Instant>,
    latest_resources: Option<SriResourceSnapshot>,
    latest_vram: Option<VramTelemetry>,
    last_alerts: HashMap<ResourceKind, Instant>,
    last_signals: HashMap<String, Instant>,
    last_function_stubs: HashMap<String, FunctionStubEmission>,
}

#[derive(Clone, Copy)]
struct VramTelemetry {
    used_mb: u64,
    total_mb: u64,
    updated_at: Instant,
}

#[derive(Clone, Copy)]
struct FunctionStubEmission {
    status: FunctionStubStatus,
    emitted_at: Instant,
}

pub struct SriActor {
    state: SriState,
    event_tx: broadcast::Sender<SriEvent>,
}

impl SriActor {
    pub fn new(registry: SriRegistry) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            state: SriState {
                registry,
                runtime: Arc::new(RwLock::new(SriRuntimeState::default())),
            },
            event_tx,
        }
    }

    pub fn state(&self) -> SriState {
        self.state.clone()
    }

    pub fn event_sender(&self) -> broadcast::Sender<SriEvent> {
        self.event_tx.clone()
    }

    pub fn start(&self, bus: Arc<EventBus>) {
        for node in scaffold_builtin_nodes() {
            self.state.registry.register(node);
        }
        self.state.registry.attach_notifier(self.event_tx.clone());
        self.state.registry.broadcast_tree_snapshot();

        let state = self.state.clone();
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            translate_bus_events(bus, state, event_tx).await;
        });

        let state = self.state.clone();
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            auto_close_shelves(state, event_tx).await;
        });

        let state = self.state.clone();
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            resource_monitor(state, event_tx).await;
        });
    }
}

impl SriState {
    pub fn snapshot(&self) -> SriSnapshot {
        let runtime = self.runtime.read().expect("sri runtime lock poisoned");
        let mut open_shelves = runtime.open_shelves.iter().cloned().collect::<Vec<_>>();
        open_shelves.sort();

        SriSnapshot {
            tree: self.registry.get_tree(),
            nodes: self.registry.nodes(),
            open_shelves,
            latest_resources: runtime.latest_resources.clone(),
        }
    }

    pub fn registry(&self) -> SriRegistry {
        self.registry.clone()
    }
}

async fn translate_bus_events(
    bus: Arc<EventBus>,
    state: SriState,
    event_tx: broadcast::Sender<SriEvent>,
) {
    let mut rx = bus.subscribe_broadcast();

    loop {
        match rx.recv().await {
            Ok(event) => handle_bus_event(&state, &event_tx, event),
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn handle_bus_event(state: &SriState, event_tx: &broadcast::Sender<SriEvent>, event: Event) {
    match event {
        Event::System(SystemEvent::ActorReady { actor_name }) => {
            if let Some(shelf_path) = actor_health_path(&actor_name) {
                emit_health_change(state, event_tx, shelf_path, HealthStatus::Active);
            }
        }
        Event::System(SystemEvent::ActorFailed { actor, reason }) => {
            if let Some(shelf_path) = actor_health_path(&actor) {
                emit_health_change(state, event_tx, shelf_path, HealthStatus::Degraded);
            }

            emit_signal(
                state,
                event_tx,
                SignalSource::Fault,
                format!("actor failed: {} ({})", actor, clean_summary(&reason)),
            );

            if let Some(resource) = infer_failure_resource(&reason) {
                let threshold = alert_threshold(resource);
                if let Some(value) = latest_resource_value(state, resource) {
                    emit_resource_alert(state, event_tx, resource, value, threshold);
                }
            }
        }
        Event::System(SystemEvent::BootComplete) => {
            mark_activity(state, "environment.system");
            open_shelf(
                state,
                event_tx,
                "environment.system",
                "system.boot_complete",
            );
            emit_signal(
                state,
                event_tx,
                SignalSource::Environment,
                "runtime ready".to_string(),
            );
        }
        Event::System(SystemEvent::VramUsageUpdated {
            used_mb, total_mb, ..
        }) => {
            state
                .runtime
                .write()
                .expect("sri runtime lock poisoned")
                .latest_vram = Some(VramTelemetry {
                used_mb: used_mb as u64,
                total_mb: total_mb as u64,
                updated_at: Instant::now(),
            });
        }
        Event::Speech(SpeechEvent::TranscriptionCompleted { text, .. }) => {
            mark_activity(state, "perception.hearing");
            open_shelf(
                state,
                event_tx,
                "perception.hearing",
                "speech.transcription_completed",
            );
            emit_signal(
                state,
                event_tx,
                SignalSource::Perception,
                format!("heard: \"{}\"", clean_summary(&text)),
            );
        }
        Event::Speech(SpeechEvent::SpeakingStarted { .. }) => {
            mark_activity(state, "expression.voice");
            open_shelf(
                state,
                event_tx,
                "expression.voice",
                "speech.speaking_started",
            );
            emit_signal(
                state,
                event_tx,
                SignalSource::Expression,
                "voice playback started".to_string(),
            );
        }
        Event::Speech(SpeechEvent::SpeakingCompleted { .. }) => {
            mark_activity(state, "expression.voice");
            emit_signal(
                state,
                event_tx,
                SignalSource::Expression,
                "voice playback completed".to_string(),
            );
        }
        Event::Inference(InferenceEvent::InferenceSentenceReady { text, .. }) => {
            mark_activity(state, "expression.language");
            open_shelf(
                state,
                event_tx,
                "expression.language",
                "inference.sentence_ready",
            );
            emit_signal(
                state,
                event_tx,
                SignalSource::Expression,
                clean_summary(&text),
            );
        }
        Event::Inference(InferenceEvent::InferenceStreamCompleted { token_count, .. }) => {
            mark_activity(state, "expression.language");
            emit_signal(
                state,
                event_tx,
                SignalSource::Expression,
                format!("response complete ({} tokens)", token_count),
            );
        }
        Event::Memory(MemoryEvent::MemoryWriteCompleted { .. })
        | Event::Memory(MemoryEvent::IngestCompleted { .. }) => {
            mark_activity(state, "cognition.memory");
            open_shelf(
                state,
                event_tx,
                "cognition.memory",
                "memory.write_completed",
            );
            emit_signal(
                state,
                event_tx,
                SignalSource::Cognition,
                "memory write stored".to_string(),
            );
        }
        Event::Memory(MemoryEvent::MemoryQueryResponse { chunks, .. })
        | Event::Memory(MemoryEvent::QueryCompleted { chunks, .. }) => {
            mark_activity(state, "cognition.memory");
            open_shelf(
                state,
                event_tx,
                "cognition.memory",
                "memory.query_completed",
            );
            emit_signal(
                state,
                event_tx,
                SignalSource::Cognition,
                format!("memory recall: {} chunks", chunks.len()),
            );
        }
        Event::Memory(MemoryEvent::ContextQueryCompleted(response)) => {
            mark_activity(state, "cognition.memory");
            open_shelf(
                state,
                event_tx,
                "cognition.memory",
                "memory.context_query_completed",
            );
            emit_signal(
                state,
                event_tx,
                SignalSource::Cognition,
                format!("memory recall: {} chunks", response.chunks.len()),
            );
        }
        Event::CTP(ctp_event) => match ctp_event.as_ref() {
            CTPEvent::ThoughtEventTriggered(snapshot) => {
                mark_activity(state, "cognition.thought");
                open_shelf(
                    state,
                    event_tx,
                    "cognition.thought",
                    "ctp.thought_triggered",
                );

                let title = snapshot
                    .active_app
                    .window_title
                    .as_deref()
                    .unwrap_or("untitled");
                emit_signal(
                    state,
                    event_tx,
                    SignalSource::Cognition,
                    format!(
                        "focus: {} / {}",
                        clean_summary(&snapshot.active_app.app_name),
                        clean_summary(title)
                    ),
                );

                if snapshot.visual_context.is_none() {
                    emit_function_stub(
                        state,
                        event_tx,
                        "perception.sight.capture_context",
                        FunctionStubStatus::Unavailable,
                    );
                }
            }
            CTPEvent::ContextSnapshotReady(snapshot) if snapshot.visual_context.is_none() => {
                emit_function_stub(
                    state,
                    event_tx,
                    "perception.sight.analyze_scene",
                    FunctionStubStatus::Unavailable,
                );
            }
            CTPEvent::ContextSnapshotReady(_) => {}
            _ => {}
        },
        Event::Soul(SoulEvent::PersonalityUpdated { metadata, .. }) => {
            mark_activity(state, "identity.soul");
            open_shelf(state, event_tx, "identity.soul", "soul.personality_updated");
            emit_signal(
                state,
                event_tx,
                SignalSource::Identity,
                format!(
                    "warmth {}, verbosity {}",
                    metadata.warmth.as_str(),
                    metadata.verbosity.as_str()
                ),
            );
        }
        _ => {}
    }
}

async fn auto_close_shelves(state: SriState, event_tx: broadcast::Sender<SriEvent>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.tick().await;

    loop {
        interval.tick().await;
        let now = Instant::now();
        let stale_paths = {
            let runtime = state.runtime.read().expect("sri runtime lock poisoned");
            runtime
                .open_shelves
                .iter()
                .filter(|path| {
                    runtime
                        .last_activity
                        .get(*path)
                        .map(|last| now.duration_since(*last) >= AUTO_CLOSE_AFTER)
                        .unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>()
        };

        for path in stale_paths {
            if close_shelf(&state, &path) {
                let _ = event_tx.send(SriEvent::TreeNavigation {
                    shelf_path: path,
                    action: TreeAction::Close,
                    triggered_by: "auto-close".to_string(),
                });
            }
        }
    }
}

async fn resource_monitor(state: SriState, event_tx: broadcast::Sender<SriEvent>) {
    let pid = get_current_pid().ok();
    let mut system = System::new_all();
    let mut interval = tokio::time::interval(RESOURCE_REFRESH_INTERVAL);
    interval.tick().await;

    loop {
        interval.tick().await;

        system.refresh_memory();
        system.refresh_cpu_usage();
        if let Some(pid) = pid {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing().with_memory().with_cpu(),
            );
        }

        let (total_ram_mb, total_cpu_pct) = if let Some(pid) = pid {
            if let Some(process) = system.process(pid) {
                let ram_mb = bytes_to_mb(process.memory());
                let cpu_count = system.cpus().len().max(1) as f32;
                let cpu_pct = (process.cpu_usage() / cpu_count).clamp(0.0, 100.0);
                (ram_mb, cpu_pct)
            } else {
                (0, 0.0)
            }
        } else {
            (0, 0.0)
        };

        let vram = current_vram_telemetry(&state);
        let resource_snapshot = SriResourceSnapshot {
            total_ram_mb,
            total_cpu_pct,
            vram_used_mb: vram.map(|sample| sample.used_mb),
            vram_total_mb: vram.map(|sample| sample.total_mb),
        };

        {
            state
                .runtime
                .write()
                .expect("sri runtime lock poisoned")
                .latest_resources = Some(resource_snapshot.clone());
        }

        let _ = event_tx.send(SriEvent::ResourceSnapshot(resource_snapshot.clone()));

        if total_ram_mb as f32 > alert_threshold(ResourceKind::Ram) {
            emit_resource_alert(
                &state,
                &event_tx,
                ResourceKind::Ram,
                total_ram_mb as f32,
                alert_threshold(ResourceKind::Ram),
            );
        }
        if total_cpu_pct > alert_threshold(ResourceKind::Cpu) {
            emit_resource_alert(
                &state,
                &event_tx,
                ResourceKind::Cpu,
                total_cpu_pct,
                alert_threshold(ResourceKind::Cpu),
            );
        }
        if let (Some(used_mb), Some(total_mb)) = (
            resource_snapshot.vram_used_mb,
            resource_snapshot.vram_total_mb,
        ) && total_mb > 0
        {
            let vram_pct = (used_mb as f32 / total_mb as f32 * 100.0).clamp(0.0, 100.0);
            if vram_pct > alert_threshold(ResourceKind::Vram) {
                emit_resource_alert(
                    &state,
                    &event_tx,
                    ResourceKind::Vram,
                    vram_pct,
                    alert_threshold(ResourceKind::Vram),
                );
            }
        }
    }
}

fn current_vram_telemetry(state: &SriState) -> Option<VramTelemetry> {
    let runtime = state.runtime.read().expect("sri runtime lock poisoned");
    let now = Instant::now();
    runtime
        .latest_vram
        .filter(|vram| now.duration_since(vram.updated_at) <= VRAM_STALE_AFTER)
}

fn actor_health_path(actor_name: &str) -> Option<&'static str> {
    match actor_name {
        "soul" => Some("identity.soul"),
        "stt" => Some("perception.hearing"),
        "memory" => Some("cognition.memory"),
        "ctp" => Some("cognition.thought"),
        "tts" => Some("expression.voice"),
        "inference" => Some("expression.language"),
        "platform" => Some("environment.system"),
        _ => None,
    }
}

fn emit_health_change(
    state: &SriState,
    event_tx: &broadcast::Sender<SriEvent>,
    shelf_path: &str,
    new_status: HealthStatus,
) {
    if let Some((old, new)) = state.registry.set_health(shelf_path, new_status) {
        let _ = event_tx.send(SriEvent::NodeHealthChanged {
            shelf_path: shelf_path.to_string(),
            old,
            new,
        });
    }
}

fn emit_signal(
    state: &SriState,
    event_tx: &broadcast::Sender<SriEvent>,
    source: SignalSource,
    summary: String,
) {
    let summary = truncate_chars(&summary, MAX_SIGNAL_SUMMARY_CHARS);
    if !should_emit_signal(state, &summary, Instant::now()) {
        return;
    }

    let _ = event_tx.send(SriEvent::SignalReceived {
        source,
        summary,
        timestamp: Utc::now(),
    });
}

fn emit_function_stub(
    state: &SriState,
    event_tx: &broadcast::Sender<SriEvent>,
    path: &str,
    status: FunctionStubStatus,
) {
    if !should_emit_function_stub(state, path, status, Instant::now()) {
        return;
    }

    let _ = event_tx.send(SriEvent::FunctionCallStub {
        path: path.to_string(),
        status,
    });
}

fn should_emit_signal(state: &SriState, summary: &str, now: Instant) -> bool {
    let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");

    if runtime
        .last_signals
        .get(summary)
        .is_some_and(|last| now.duration_since(*last) < SIGNAL_DEDUP_WINDOW)
    {
        return false;
    }

    runtime.last_signals.insert(summary.to_string(), now);
    true
}

fn should_emit_function_stub(
    state: &SriState,
    path: &str,
    status: FunctionStubStatus,
    now: Instant,
) -> bool {
    let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");

    if let Some(previous) = runtime.last_function_stubs.get(path).copied()
        && previous.status == status
        && matches!(status, FunctionStubStatus::Unavailable)
        && now.duration_since(previous.emitted_at) < UNAVAILABLE_STUB_DEDUP_WINDOW
    {
        return false;
    }

    runtime.last_function_stubs.insert(
        path.to_string(),
        FunctionStubEmission {
            status,
            emitted_at: now,
        },
    );
    true
}

fn open_shelf(
    state: &SriState,
    event_tx: &broadcast::Sender<SriEvent>,
    shelf_path: &str,
    triggered_by: &str,
) {
    let became_open = {
        let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
        runtime
            .last_activity
            .insert(shelf_path.to_string(), Instant::now());
        runtime.open_shelves.insert(shelf_path.to_string())
    };

    if became_open {
        let _ = event_tx.send(SriEvent::TreeNavigation {
            shelf_path: shelf_path.to_string(),
            action: TreeAction::Open,
            triggered_by: triggered_by.to_string(),
        });
    }
}

fn close_shelf(state: &SriState, shelf_path: &str) -> bool {
    let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
    runtime.last_activity.remove(shelf_path);
    runtime.open_shelves.remove(shelf_path)
}

fn mark_activity(state: &SriState, shelf_path: &str) {
    state
        .runtime
        .write()
        .expect("sri runtime lock poisoned")
        .last_activity
        .insert(shelf_path.to_string(), Instant::now());
}

fn emit_resource_alert(
    state: &SriState,
    event_tx: &broadcast::Sender<SriEvent>,
    resource: ResourceKind,
    value: f32,
    threshold: f32,
) {
    let now = Instant::now();
    let should_emit = {
        let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
        if runtime
            .last_alerts
            .get(&resource)
            .is_some_and(|last| now.duration_since(*last) < ALERT_COOLDOWN)
        {
            false
        } else {
            runtime.last_alerts.insert(resource, now);
            true
        }
    };

    if should_emit {
        let _ = event_tx.send(SriEvent::ResourceAlert {
            actor: "process".to_string(),
            resource,
            value,
            threshold,
        });
    }
}

fn latest_resource_value(state: &SriState, resource: ResourceKind) -> Option<f32> {
    let runtime = state.runtime.read().expect("sri runtime lock poisoned");
    let latest = runtime.latest_resources.as_ref()?;
    match resource {
        ResourceKind::Ram => Some(latest.total_ram_mb as f32),
        ResourceKind::Cpu => Some(latest.total_cpu_pct),
        ResourceKind::Vram => match (latest.vram_used_mb, latest.vram_total_mb) {
            (Some(used_mb), Some(total_mb)) if total_mb > 0 => {
                Some((used_mb as f32 / total_mb as f32 * 100.0).clamp(0.0, 100.0))
            }
            _ => None,
        },
    }
}

fn infer_failure_resource(reason: &str) -> Option<ResourceKind> {
    let reason = reason.to_ascii_lowercase();
    if reason.contains("oom") || reason.contains("memory") || reason.contains("ram") {
        Some(ResourceKind::Ram)
    } else if reason.contains("vram") || reason.contains("cuda") || reason.contains("gpu") {
        Some(ResourceKind::Vram)
    } else if reason.contains("cpu") || reason.contains("timeout") || reason.contains("starved") {
        Some(ResourceKind::Cpu)
    } else {
        None
    }
}

fn alert_threshold(resource: ResourceKind) -> f32 {
    match resource {
        ResourceKind::Ram => 3.0 * 1024.0,
        ResourceKind::Cpu => 80.0,
        ResourceKind::Vram => 90.0,
    }
}

fn clean_summary(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, MAX_SIGNAL_SUMMARY_CHARS)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }

    let truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{}…", truncated)
}

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_signal_summaries_are_deduplicated_for_five_seconds() {
        let actor = SriActor::new(SriRegistry::new());
        let state = actor.state();
        let start = Instant::now();

        assert!(should_emit_signal(&state, "focus: vscode / shell", start));
        assert!(!should_emit_signal(
            &state,
            "focus: vscode / shell",
            start + Duration::from_secs(4),
        ));
        assert!(should_emit_signal(
            &state,
            "focus: vscode / shell",
            start + Duration::from_secs(6),
        ));
    }

    #[test]
    fn unavailable_function_stubs_are_deduplicated_for_sixty_seconds() {
        let actor = SriActor::new(SriRegistry::new());
        let state = actor.state();
        let start = Instant::now();

        assert!(should_emit_function_stub(
            &state,
            "perception.sight.analyze_scene",
            FunctionStubStatus::Unavailable,
            start,
        ));
        assert!(!should_emit_function_stub(
            &state,
            "perception.sight.analyze_scene",
            FunctionStubStatus::Unavailable,
            start + Duration::from_secs(30),
        ));
        assert!(should_emit_function_stub(
            &state,
            "perception.sight.analyze_scene",
            FunctionStubStatus::Unavailable,
            start + Duration::from_secs(61),
        ));
    }
}
