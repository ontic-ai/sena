use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use bus::{CTPEvent, Event, EventBus, InferenceEvent, MemoryEvent, SoulEvent, SpeechEvent, SystemEvent};
use chrono::Utc;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid};
use tokio::sync::broadcast;
use crate::builtin_nodes::scaffold_builtin_nodes;
use crate::{
    ActorResourceEstimate, FunctionStubStatus, HealthStatus, ResourceKind, SignalSource,
    SriEvent, SriRegistry, SriResourceSnapshot, SriSnapshot, TreeAction,
};

const AUTO_CLOSE_AFTER: Duration = Duration::from_secs(10);
const RESOURCE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const ALERT_COOLDOWN: Duration = Duration::from_secs(10);
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
    last_alerts: HashMap<(String, ResourceKind), Instant>,
    memory_observation_count: usize,
    inference_active_until: Option<Instant>,
    transcription_active_until: Option<Instant>,
    synthesis_active_until: Option<Instant>,
    thought_active_until: Option<Instant>,
}

#[derive(Clone, Copy)]
struct VramTelemetry {
    used_mb: u64,
    total_mb: u64,
    updated_at: Instant,
}

#[derive(Clone, Copy)]
struct ActivitySnapshot {
    inference_active: bool,
    transcription_active: bool,
    synthesis_active: bool,
    thought_active: bool,
    memory_observation_count: usize,
    vram: Option<VramTelemetry>,
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
                event_tx,
                SignalSource::Fault,
                format!("actor failed: {} ({})", actor, clean_summary(&reason)),
            );

            if let Some(resource) = infer_failure_resource(&reason) {
                let value = latest_resource_value(state, &actor, resource).unwrap_or(100.0);
                let threshold = alert_threshold(resource);
                emit_resource_alert(state, event_tx, actor, resource, value, threshold);
            }
        }
        Event::System(SystemEvent::BootComplete) => {
            mark_activity(state, "environment.system");
            open_shelf(state, event_tx, "environment.system", "system.boot_complete");
            emit_signal(event_tx, SignalSource::Environment, "runtime ready".to_string());
        }
        Event::System(SystemEvent::VramUsageUpdated {
            used_mb,
            total_mb,
            ..
        }) => {
            state.runtime.write().expect("sri runtime lock poisoned").latest_vram = Some(
                VramTelemetry {
                    used_mb: used_mb as u64,
                    total_mb: total_mb as u64,
                    updated_at: Instant::now(),
                },
            );
        }
        Event::Speech(SpeechEvent::TranscriptionCompleted { text, .. }) => {
            set_until(&state, ActivityKind::Transcription, Instant::now() + AUTO_CLOSE_AFTER);
            mark_activity(state, "perception.hearing");
            open_shelf(state, event_tx, "perception.hearing", "speech.transcription_completed");
            emit_signal(
                event_tx,
                SignalSource::Perception,
                format!("heard: \"{}\"", clean_summary(&text)),
            );
        }
        Event::Speech(SpeechEvent::SpeakingStarted { .. }) => {
            set_until(&state, ActivityKind::Synthesis, Instant::now() + AUTO_CLOSE_AFTER);
            mark_activity(state, "expression.voice");
            open_shelf(state, event_tx, "expression.voice", "speech.speaking_started");
            emit_signal(
                event_tx,
                SignalSource::Expression,
                "voice playback started".to_string(),
            );
        }
        Event::Speech(SpeechEvent::SpeakingCompleted { .. }) => {
            set_until(
                &state,
                ActivityKind::Synthesis,
                Instant::now() + Duration::from_secs(2),
            );
            mark_activity(state, "expression.voice");
            emit_signal(
                event_tx,
                SignalSource::Expression,
                "voice playback completed".to_string(),
            );
        }
        Event::Inference(InferenceEvent::InferenceSentenceReady { text, .. }) => {
            set_until(&state, ActivityKind::Inference, Instant::now() + AUTO_CLOSE_AFTER);
            mark_activity(state, "expression.language");
            open_shelf(
                state,
                event_tx,
                "expression.language",
                "inference.sentence_ready",
            );
            emit_signal(event_tx, SignalSource::Expression, clean_summary(&text));
        }
        Event::Inference(InferenceEvent::InferenceStreamCompleted { token_count, .. }) => {
            set_until(&state, ActivityKind::Inference, Instant::now() + Duration::from_secs(4));
            mark_activity(state, "expression.language");
            emit_signal(
                event_tx,
                SignalSource::Expression,
                format!("response complete ({} tokens)", token_count),
            );
        }
        Event::Memory(MemoryEvent::MemoryWriteCompleted { .. })
        | Event::Memory(MemoryEvent::IngestCompleted { .. }) => {
            increment_memory_observation(state, 1);
            mark_activity(state, "cognition.memory");
            open_shelf(state, event_tx, "cognition.memory", "memory.write_completed");
            emit_signal(
                event_tx,
                SignalSource::Cognition,
                "memory write stored".to_string(),
            );
        }
        Event::Memory(MemoryEvent::MemoryQueryResponse { chunks, .. })
        | Event::Memory(MemoryEvent::QueryCompleted { chunks, .. }) => {
            observe_memory_chunks(state, chunks.len());
            mark_activity(state, "cognition.memory");
            open_shelf(state, event_tx, "cognition.memory", "memory.query_completed");
            emit_signal(
                event_tx,
                SignalSource::Cognition,
                format!("memory recall: {} chunks", chunks.len()),
            );
        }
        Event::Memory(MemoryEvent::ContextQueryCompleted(response)) => {
            observe_memory_chunks(state, response.chunks.len());
            mark_activity(state, "cognition.memory");
            open_shelf(state, event_tx, "cognition.memory", "memory.context_query_completed");
            emit_signal(
                event_tx,
                SignalSource::Cognition,
                format!("memory recall: {} chunks", response.chunks.len()),
            );
        }
        Event::CTP(ctp_event) => match ctp_event.as_ref() {
            CTPEvent::ThoughtEventTriggered(snapshot) => {
                set_until(&state, ActivityKind::Thought, Instant::now() + AUTO_CLOSE_AFTER);
                mark_activity(state, "cognition.thought");
                open_shelf(state, event_tx, "cognition.thought", "ctp.thought_triggered");

                let title = snapshot.active_app.window_title.as_deref().unwrap_or("untitled");
                emit_signal(
                    event_tx,
                    SignalSource::Cognition,
                    format!(
                        "focus: {} / {}",
                        clean_summary(&snapshot.active_app.app_name),
                        clean_summary(title)
                    ),
                );

                if snapshot.visual_context.is_none() {
                    let _ = event_tx.send(SriEvent::FunctionCallStub {
                        path: "perception.sight.capture_context".to_string(),
                        status: FunctionStubStatus::Unavailable,
                    });
                }
            }
            CTPEvent::ContextSnapshotReady(snapshot) => {
                if snapshot.visual_context.is_none() {
                    let _ = event_tx.send(SriEvent::FunctionCallStub {
                        path: "perception.sight.analyze_scene".to_string(),
                        status: FunctionStubStatus::Unavailable,
                    });
                }
            }
            _ => {}
        },
        Event::Soul(SoulEvent::PersonalityUpdated { metadata, .. }) => {
            mark_activity(state, "identity.soul");
            open_shelf(state, event_tx, "identity.soul", "soul.personality_updated");
            emit_signal(
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

        let activity = snapshot_activity(&state);
        let estimates = estimate_actor_resources(total_ram_mb, total_cpu_pct, activity);
        let resource_snapshot = SriResourceSnapshot {
            total_ram_mb,
            total_cpu_pct,
            vram_used_mb: activity.vram.map(|vram| vram.used_mb),
            vram_total_mb: activity.vram.map(|vram| vram.total_mb),
            actors: estimates,
        };

        {
            state
                .runtime
                .write()
                .expect("sri runtime lock poisoned")
                .latest_resources = Some(resource_snapshot.clone());
        }

        let _ = event_tx.send(SriEvent::ResourceSnapshot(resource_snapshot.clone()));

        for estimate in &resource_snapshot.actors {
            if estimate.ram_mb as f32 > alert_threshold(ResourceKind::Ram) {
                emit_resource_alert(
                    &state,
                    &event_tx,
                    estimate.actor_name.clone(),
                    ResourceKind::Ram,
                    estimate.ram_mb as f32,
                    alert_threshold(ResourceKind::Ram),
                );
            }
            if estimate.cpu_pct > alert_threshold(ResourceKind::Cpu) {
                emit_resource_alert(
                    &state,
                    &event_tx,
                    estimate.actor_name.clone(),
                    ResourceKind::Cpu,
                    estimate.cpu_pct,
                    alert_threshold(ResourceKind::Cpu),
                );
            }
            if let Some(vram_pct) = estimate.vram_pct
                && vram_pct > alert_threshold(ResourceKind::Vram)
            {
                emit_resource_alert(
                    &state,
                    &event_tx,
                    estimate.actor_name.clone(),
                    ResourceKind::Vram,
                    vram_pct,
                    alert_threshold(ResourceKind::Vram),
                );
            }
        }
    }
}

fn snapshot_activity(state: &SriState) -> ActivitySnapshot {
    let runtime = state.runtime.read().expect("sri runtime lock poisoned");
    let now = Instant::now();
    ActivitySnapshot {
        inference_active: runtime
            .inference_active_until
            .is_some_and(|until| until > now),
        transcription_active: runtime
            .transcription_active_until
            .is_some_and(|until| until > now),
        synthesis_active: runtime
            .synthesis_active_until
            .is_some_and(|until| until > now),
        thought_active: runtime.thought_active_until.is_some_and(|until| until > now),
        memory_observation_count: runtime.memory_observation_count,
        vram: runtime.latest_vram.filter(|vram| now.duration_since(vram.updated_at) <= Duration::from_secs(5)),
    }
}

fn estimate_actor_resources(
    total_ram_mb: u64,
    total_cpu_pct: f32,
    activity: ActivitySnapshot,
) -> Vec<ActorResourceEstimate> {
    let memory_scale = (activity.memory_observation_count as f32 / 100.0).clamp(0.0, 3.0);

    let ram_weights = vec![
        (
            "inference",
            if activity.inference_active { 4.5 } else { 2.0 },
            "weighted by recent language activity and GPU pressure".to_string(),
        ),
        (
            "speech-stt",
            if activity.transcription_active { 1.6 } else { 0.5 },
            "weighted by recent transcription activity".to_string(),
        ),
        (
            "speech-tts",
            if activity.synthesis_active { 1.2 } else { 0.4 },
            "weighted by recent voice playback".to_string(),
        ),
        (
            "memory",
            0.8 + memory_scale,
            format!(
                "estimated from {} observed memory chunks",
                activity.memory_observation_count
            ),
        ),
        (
            "ctp",
            if activity.thought_active { 1.1 } else { 0.5 },
            "weighted by recent context assembly activity".to_string(),
        ),
        ("soul", 0.35, "steady identity metadata footprint".to_string()),
        (
            "platform",
            0.4,
            "steady OS observation and event routing".to_string(),
        ),
        ("sri", 0.3, "registry and visualization bookkeeping".to_string()),
        (
            "runtime",
            0.6,
            "bus, supervision, and daemon orchestration".to_string(),
        ),
    ];

    let cpu_weights = vec![
        (
            "inference",
            if activity.inference_active { 5.0 } else { 1.5 },
            "weighted by recent language activity and GPU pressure".to_string(),
        ),
        (
            "speech-stt",
            if activity.transcription_active { 2.0 } else { 0.3 },
            "weighted by recent transcription activity".to_string(),
        ),
        (
            "speech-tts",
            if activity.synthesis_active { 1.8 } else { 0.2 },
            "weighted by recent voice playback".to_string(),
        ),
        (
            "memory",
            0.7 + memory_scale * 0.5,
            format!(
                "estimated from {} observed memory chunks",
                activity.memory_observation_count
            ),
        ),
        (
            "ctp",
            if activity.thought_active { 1.4 } else { 0.4 },
            "weighted by recent context assembly activity".to_string(),
        ),
        ("soul", 0.2, "steady identity metadata footprint".to_string()),
        (
            "platform",
            0.35,
            "steady OS observation and event routing".to_string(),
        ),
        ("sri", 0.45, "registry and visualization bookkeeping".to_string()),
        (
            "runtime",
            0.75,
            "bus, supervision, and daemon orchestration".to_string(),
        ),
    ];

    let total_ram_weight = ram_weights.iter().map(|(_, weight, _)| *weight).sum::<f32>();
    let total_cpu_weight = cpu_weights.iter().map(|(_, weight, _)| *weight).sum::<f32>();
    let total_vram_pct = activity
        .vram
        .and_then(|vram| percentage(vram.used_mb as f32, vram.total_mb as f32));

    ram_weights
        .iter()
        .zip(cpu_weights.iter())
        .map(|((actor_name, ram_weight, ram_basis), (_, cpu_weight, _))| {
            let ram_mb = if total_ram_mb == 0 || total_ram_weight == 0.0 {
                0
            } else {
                ((total_ram_mb as f32) * (*ram_weight / total_ram_weight)).round() as u64
            };
            let cpu_pct = if total_cpu_pct == 0.0 || total_cpu_weight == 0.0 {
                0.0
            } else {
                total_cpu_pct * (*cpu_weight / total_cpu_weight)
            };
            let vram_pct = total_vram_pct.map(|overall| actor_vram_pct(*actor_name, overall, activity));

            ActorResourceEstimate {
                actor_name: (*actor_name).to_string(),
                ram_mb,
                cpu_pct,
                vram_pct,
                basis: ram_basis.clone(),
            }
        })
        .collect()
}

fn actor_vram_pct(actor_name: &str, overall_vram_pct: f32, activity: ActivitySnapshot) -> f32 {
    let share = match actor_name {
        "inference" => {
            if activity.inference_active {
                0.78
            } else {
                0.60
            }
        }
        "speech-stt" => {
            if activity.transcription_active {
                0.12
            } else {
                0.04
            }
        }
        "speech-tts" => {
            if activity.synthesis_active {
                0.08
            } else {
                0.03
            }
        }
        "runtime" => 0.07,
        _ => 0.0,
    };

    (overall_vram_pct * share).clamp(0.0, 100.0)
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

fn emit_signal(event_tx: &broadcast::Sender<SriEvent>, source: SignalSource, summary: String) {
    let _ = event_tx.send(SriEvent::SignalReceived {
        source,
        summary: truncate_chars(&summary, MAX_SIGNAL_SUMMARY_CHARS),
        timestamp: Utc::now(),
    });
}

fn open_shelf(
    state: &SriState,
    event_tx: &broadcast::Sender<SriEvent>,
    shelf_path: &str,
    triggered_by: &str,
) {
    let became_open = {
        let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
        runtime.last_activity.insert(shelf_path.to_string(), Instant::now());
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

fn increment_memory_observation(state: &SriState, amount: usize) {
    let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
    runtime.memory_observation_count = runtime.memory_observation_count.saturating_add(amount);
}

fn observe_memory_chunks(state: &SriState, chunk_count: usize) {
    let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
    runtime.memory_observation_count = runtime.memory_observation_count.max(chunk_count);
}

fn emit_resource_alert(
    state: &SriState,
    event_tx: &broadcast::Sender<SriEvent>,
    actor: impl Into<String>,
    resource: ResourceKind,
    value: f32,
    threshold: f32,
) {
    let actor = actor.into();
    let now = Instant::now();
    let should_emit = {
        let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
        let key = (actor.clone(), resource);
        if runtime
            .last_alerts
            .get(&key)
            .is_some_and(|last| now.duration_since(*last) < ALERT_COOLDOWN)
        {
            false
        } else {
            runtime.last_alerts.insert(key, now);
            true
        }
    };

    if should_emit {
        let _ = event_tx.send(SriEvent::ResourceAlert {
            actor,
            resource,
            value,
            threshold,
        });
    }
}

fn latest_resource_value(state: &SriState, actor: &str, resource: ResourceKind) -> Option<f32> {
    let runtime = state.runtime.read().expect("sri runtime lock poisoned");
    let latest = runtime.latest_resources.as_ref()?;
    let estimate = latest.actors.iter().find(|estimate| estimate.actor_name == actor)?;
    match resource {
        ResourceKind::Ram => Some(estimate.ram_mb as f32),
        ResourceKind::Cpu => Some(estimate.cpu_pct),
        ResourceKind::Vram => estimate.vram_pct,
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
        ResourceKind::Ram => 768.0,
        ResourceKind::Cpu => 65.0,
        ResourceKind::Vram => 70.0,
    }
}

#[derive(Clone, Copy)]
enum ActivityKind {
    Inference,
    Transcription,
    Synthesis,
    Thought,
}

fn set_until(state: &SriState, kind: ActivityKind, until: Instant) {
    let mut runtime = state.runtime.write().expect("sri runtime lock poisoned");
    match kind {
        ActivityKind::Inference => runtime.inference_active_until = Some(until),
        ActivityKind::Transcription => runtime.transcription_active_until = Some(until),
        ActivityKind::Synthesis => runtime.synthesis_active_until = Some(until),
        ActivityKind::Thought => runtime.thought_active_until = Some(until),
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

    let truncated = text.chars().take(max_chars.saturating_sub(1)).collect::<String>();
    format!("{}…", truncated)
}

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

fn percentage(part: f32, total: f32) -> Option<f32> {
    if total <= 0.0 {
        None
    } else {
        Some((part / total * 100.0).clamp(0.0, 100.0))
    }
}