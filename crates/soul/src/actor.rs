//! SoulActor: owns the EncryptedDb and handles all Soul subsystem events.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bus::events::soul::{
    SoulEventLogged, SoulReadCompleted, SoulSummary, SoulSummaryRequested, SoulWriteRequest,
};
use bus::events::transparency::SoulSummaryForTransparency;
use bus::events::{CTPEvent, InferenceEvent, MemoryEvent, PlatformEvent};
use bus::{Actor, ActorError, Event, EventBus, SoulEvent, SystemEvent};
use redb::{ReadableDatabase, ReadableTable};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;

use crate::{
    encrypted_db::EncryptedDb,
    error::SoulError,
    schema::{self, EVENT_LOG, IDENTITY_SIGNALS, USER_IDENTITY},
};

const SOUL_ACTOR_NAME: &str = "soul";
const SOUL_CHANNEL_CAPACITY: usize = 256;

/// Actor that owns the SoulBox encrypted database.
pub struct SoulActor {
    db_path: std::path::PathBuf,
    master_key: Option<crypto::MasterKey>,
    db: Option<EncryptedDb>,
    next_event_id: u64,
    bus: Option<Arc<EventBus>>,
    broadcast_rx: Option<broadcast::Receiver<Event>>,
    directed_rx: Option<mpsc::Receiver<Event>>,
    task_set: JoinSet<()>,
}

impl SoulActor {
    pub fn new(db_path: impl Into<std::path::PathBuf>, master_key: crypto::MasterKey) -> Self {
        Self {
            db_path: db_path.into(),
            master_key: Some(master_key),
            db: None,
            next_event_id: 0,
            bus: None,
            broadcast_rx: None,
            directed_rx: None,
            task_set: JoinSet::new(),
        }
    }

    fn handle_write(
        &mut self,
        req: SoulWriteRequest,
        bus: &Arc<EventBus>,
    ) -> Result<(), SoulError> {
        let row_id = self.next_event_id;
        self.next_event_id += 1;

        let db = self
            .db
            .as_ref()
            .ok_or_else(|| SoulError::Database("not initialized".into()))?;
        let redb = db.db()?;

        let entry = format!(
            "[{}] {}",
            req.app_context.as_deref().unwrap_or("unknown"),
            req.description
        );
        let write_txn = redb.begin_write()?;
        {
            let mut log = write_txn.open_table(EVENT_LOG)?;
            log.insert(row_id, entry.as_bytes())?;
        }
        write_txn.commit()?;

        let request_id = req.request_id;
        let bus_clone = Arc::clone(bus);
        self.task_set.spawn(async move {
            let _ = bus_clone
                .broadcast(Event::Soul(SoulEvent::EventLogged(SoulEventLogged {
                    row_id,
                    request_id,
                })))
                .await;
        });
        Ok(())
    }

    fn handle_summary(
        &mut self,
        req: SoulSummaryRequested,
        bus: &Arc<EventBus>,
    ) -> Result<(), SoulError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| SoulError::Database("not initialized".into()))?;
        let redb = db.db()?;

        let read_txn = redb.begin_read()?;
        let log = read_txn.open_table(EVENT_LOG)?;

        let mut entries: Vec<String> = log
            .iter()?
            .rev()
            .take(req.max_events)
            .map(
                |r: Result<(redb::AccessGuard<'_, u64>, redb::AccessGuard<'_, &[u8]>), _>| {
                    let (_, v) = r?;
                    Ok(String::from_utf8_lossy(v.value()).into_owned())
                },
            )
            .collect::<Result<Vec<_>, redb::StorageError>>()?;

        entries.reverse();

        let signals = read_identity_signals(redb)?;
        if !signals.is_empty() {
            entries.push("[identity]".to_string());
            for (key, value) in signals.into_iter().take(20) {
                entries.push(format!("{}={}", key, value));
            }
        }

        let event_count = entries.len();
        let content = entries.join("\n");
        let request_id = req.request_id;
        let bus_clone = Arc::clone(bus);
        self.task_set.spawn(async move {
            let _ = bus_clone
                .broadcast(Event::Soul(SoulEvent::SummaryReady(SoulSummary {
                    content,
                    event_count,
                    request_id,
                })))
                .await;
        });
        Ok(())
    }

    fn handle_identity_signal(&self, key: String, value: String) -> Result<(), SoulError> {
        self.write_identity_signal(key.as_str(), value.as_str())
    }

    fn write_identity_signal(&self, key: &str, value: &str) -> Result<(), SoulError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| SoulError::Database("not initialized".into()))?;
        let redb = db.db()?;
        let write_txn = redb.begin_write()?;
        {
            let mut signals = write_txn.open_table(IDENTITY_SIGNALS)?;
            signals.insert(key, value)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    fn increment_identity_counter(&self, key: &str, delta: u64) -> Result<(), SoulError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| SoulError::Database("not initialized".into()))?;
        let redb = db.db()?;
        let write_txn = redb.begin_write()?;
        {
            let mut signals = write_txn.open_table(IDENTITY_SIGNALS)?;
            let current = signals
                .get(key)?
                .and_then(|v| v.value().parse::<u64>().ok())
                .unwrap_or(0);
            let next = current.saturating_add(delta);
            signals.insert(key, next.to_string().as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Compute TTS personality parameters from soul identity signals and emit
    /// a `PersonalityUpdated` event on the bus.
    fn emit_personality_updated(&mut self, bus: &Arc<EventBus>) -> Result<(), SoulError> {
        let personality = self.compute_personality()?;
        let bus_clone = Arc::clone(bus);
        self.task_set.spawn(async move {
            let _ = bus_clone
                .broadcast(Event::Soul(SoulEvent::PersonalityUpdated(personality)))
                .await;
        });
        Ok(())
    }

    /// Derive TTS personality parameters from stored identity signals.
    ///
    /// Returns default middle-ground values when signals are absent (new install).
    fn compute_personality(
        &self,
    ) -> Result<bus::events::soul::PersonalityUpdated, SoulError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| SoulError::Database("not initialized".into()))?;
        let redb = db.db()?;
        let signals = read_identity_signals(redb)?;

        // Default values for a fresh Soul.
        let mut rate: f32 = 1.0;
        let mut warmth: u8 = 60;
        let mut verbosity: u8 = 50;

        for (key, value) in &signals {
            match key.as_str() {
                "voice::rate" => {
                    if let Ok(v) = value.parse::<f32>() {
                        rate = v.clamp(0.5, 2.0);
                    }
                }
                "voice::warmth" => {
                    if let Ok(v) = value.parse::<u8>() {
                        warmth = v.min(100);
                    }
                }
                "voice::verbosity" => {
                    if let Ok(v) = value.parse::<u8>() {
                        verbosity = v.min(100);
                    }
                }
                // Infer from cadence: fast workers tend to prefer brisk responses.
                "work_pattern::high_cadence_count" => {
                    if let Ok(n) = value.parse::<u64>() {
                        if n > 50 {
                            rate = (rate + 0.1).min(2.0);
                            verbosity = verbosity.saturating_sub(5);
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(bus::events::soul::PersonalityUpdated {
            rate,
            warmth,
            verbosity,
        })
    }

    fn handle_initialize_with_name(
        &mut self,
        name: String,
        bus: &Arc<EventBus>,
    ) -> Result<(), SoulError> {
        // Validate: name must not be empty and max 50 chars
        if name.is_empty() || name.len() > 50 {
            return Err(SoulError::Database(format!(
                "invalid user name: must be 1-50 characters, got {} chars",
                name.len()
            )));
        }

        let db = self
            .db
            .as_ref()
            .ok_or_else(|| SoulError::Database("not initialized".into()))?;
        let redb = db.db()?;

        let write_txn = redb.begin_write()?;
        {
            let mut table = write_txn.open_table(USER_IDENTITY)?;
            table.insert("user_name", name.as_str())?;
            // Store timestamp using SystemTime with explicit fallback for system clock issues
            let timestamp = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            {
                Ok(duration) => duration.as_secs(),
                Err(_) => {
                    // System clock is before UNIX_EPOCH (rare but possible).
                    // Use deterministic fallback: 0 (UNIX_EPOCH itself).
                    0
                }
            };
            table.insert("created_at", &*timestamp.to_string())?;
        }
        write_txn.commit()?;

        // Emit NameInitialized ack on bus
        let bus_clone = Arc::clone(bus);
        let name_clone = name.clone();
        self.task_set.spawn(async move {
            let _ = bus_clone
                .broadcast(Event::Soul(SoulEvent::NameInitialized(
                    bus::events::soul::SoulNameInitialized { name: name_clone },
                )))
                .await;
        });

        Ok(())
    }

    fn absorb_platform_signal(&self, event: PlatformEvent) -> Result<(), SoulError> {
        match event {
            PlatformEvent::WindowChanged(window) => {
                let key = format!("tool_pref::{}", window.app_name.to_lowercase());
                self.increment_identity_counter(&key, 1)?;
            }
            PlatformEvent::FileEvent(file) => {
                if let Some(ext) = file.path.extension().and_then(|e| e.to_str()) {
                    let key = format!("work_pattern::file_ext::{}", ext.to_lowercase());
                    self.increment_identity_counter(&key, 1)?;
                }
            }
            PlatformEvent::KeystrokePattern(cadence) => {
                if cadence.burst_detected {
                    self.increment_identity_counter("work_pattern::burst_count", 1)?;
                }
                if cadence.events_per_minute >= 140.0 {
                    self.increment_identity_counter("work_pattern::high_cadence_count", 1)?;
                }
            }
            PlatformEvent::ClipboardChanged(_) => {}
        }
        Ok(())
    }

    fn absorb_ctp_signal(&self, event: CTPEvent) -> Result<(), SoulError> {
        match event {
            CTPEvent::ContextSnapshotReady(snapshot) => {
                if let Some(task) = snapshot.inferred_task {
                    let key = format!("task_pref::{}", task.category.to_lowercase());
                    self.increment_identity_counter(&key, 1)?;
                }
            }
            CTPEvent::ThoughtEventTriggered(_) => {
                self.increment_identity_counter("ctp::thought_trigger_count", 1)?;
            }
        }
        Ok(())
    }

    fn absorb_inference_signal(&self, event: InferenceEvent) -> Result<(), SoulError> {
        if let InferenceEvent::InferenceCompleted { text, .. } = event {
            for topic in infer_interest_topics(&text) {
                let key = format!("interest::{}", topic);
                self.increment_identity_counter(&key, 1)?;
            }
        }
        Ok(())
    }

    fn absorb_system_signal(&self, event: SystemEvent) -> Result<(), SoulError> {
        match event {
            SystemEvent::FirstBoot => {
                self.increment_identity_counter("system::first_boot_count", 1)?;
            }
            SystemEvent::BootComplete => {
                self.increment_identity_counter("system::boot_complete_count", 1)?;
            }
            SystemEvent::CliSessionClosed => {
                self.increment_identity_counter("system::cli_session_closed_count", 1)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle a transparency ReadRequested — build a redacted summary and broadcast ReadCompleted.
    fn handle_read(
        &mut self,
        req: bus::events::soul::SoulReadRequest,
        bus: &Arc<EventBus>,
    ) -> Result<(), SoulError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| SoulError::Database("not initialized".into()))?;
        let redb = db.db()?;

        let signals = read_identity_signals(redb)?;
        let mut work_patterns = vec![];
        let mut tool_preferences = vec![];
        let mut interest_clusters = vec![];
        let mut inference_cycle_count = 0usize;

        for (key, value) in &signals {
            if key.starts_with("task_pref::") {
                if let Ok(n) = value.parse::<u64>() {
                    if n > 0 {
                        work_patterns.push(key.trim_start_matches("task_pref::").to_string());
                    }
                }
            } else if key.starts_with("tool_pref::") {
                if let Ok(n) = value.parse::<u64>() {
                    if n > 0 {
                        tool_preferences.push(key.trim_start_matches("tool_pref::").to_string());
                    }
                }
            } else if key.starts_with("interest::") {
                if let Ok(n) = value.parse::<u64>() {
                    if n > 0 {
                        interest_clusters.push(key.trim_start_matches("interest::").to_string());
                    }
                }
            } else if key == "inference_cycle_count" {
                inference_cycle_count = value.parse().unwrap_or(0);
            }
        }

        // Read user_name from USER_IDENTITY table
        let user_name = redb
            .begin_read()
            .ok()
            .and_then(|txn| txn.open_table(USER_IDENTITY).ok())
            .and_then(|table| table.get("user_name").ok())
            .and_then(|opt| opt)
            .map(|g| g.value().to_string());

        let summary = SoulSummaryForTransparency {
            user_name,
            inference_cycle_count,
            work_patterns,
            tool_preferences,
            interest_clusters,
        };
        let request_id = req.request_id;
        let bus_clone = Arc::clone(bus);
        self.task_set.spawn(async move {
            let _ = bus_clone
                .broadcast(Event::Soul(SoulEvent::ReadCompleted(SoulReadCompleted {
                    summary,
                    request_id,
                })))
                .await;
        });
        Ok(())
    }
}

fn read_identity_signals(db: &redb::Database) -> Result<Vec<(String, String)>, SoulError> {
    let read_txn = db.begin_read()?;
    let table = read_txn.open_table(IDENTITY_SIGNALS)?;
    let mut rows: Vec<(String, String)> = table
        .iter()?
        .map(|entry| {
            let (k, v) = entry?;
            Ok((k.value().to_string(), v.value().to_string()))
        })
        .collect::<Result<Vec<_>, redb::StorageError>>()?;
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(rows)
}

fn infer_interest_topics(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    let mut topics = Vec::new();
    if lower.contains("rust") || lower.contains("cargo") || lower.contains("tokio") {
        topics.push("rust");
    }
    if lower.contains("llm")
        || lower.contains("embedding")
        || lower.contains("prompt")
        || lower.contains("model")
    {
        topics.push("ai");
    }
    if lower.contains("sql") || lower.contains("database") || lower.contains("redb") {
        topics.push("data");
    }
    if lower.contains("debug") || lower.contains("error") || lower.contains("trace") {
        topics.push("debugging");
    }
    topics
}

#[async_trait]
impl Actor for SoulActor {
    fn name(&self) -> &'static str {
        SOUL_ACTOR_NAME
    }

    async fn start(&mut self, bus: Arc<EventBus>) -> Result<(), ActorError> {
        let master_key = self
            .master_key
            .take()
            .ok_or_else(|| ActorError::StartupFailed("SoulActor already started".into()))?;

        // Open the encrypted DB. If decryption fails (master key rotated between runs),
        // back up the corrupt file and start fresh so the actor stays alive.
        let db = match EncryptedDb::open(&self.db_path, &master_key) {
            Ok(db) => db,
            Err(e) => {
                let reason = e.to_string();
                if reason.contains("decryption failed") || reason.contains("encryption error") {
                    // Safe recovery: back up the file before replacing it
                    if self.db_path.exists() {
                        let backup_path = self.db_path.with_extension("bak");
                        std::fs::rename(&self.db_path, &backup_path).map_err(|e| {
                            ActorError::StartupFailed(format!(
                                "failed to backup corrupt database: {}",
                                e
                            ))
                        })?;

                        // Emit recovery event to surface the data loss risk
                        let bus_clone = Arc::clone(&bus);
                        let backup_path_str = backup_path.display().to_string();
                        tokio::spawn(async move {
                            let _ = bus_clone
                                .broadcast(Event::System(SystemEvent::DatabaseRecovered {
                                    backup_path: backup_path_str,
                                }))
                                .await;
                        });
                    }
                    EncryptedDb::open(&self.db_path, &master_key)
                        .map_err(|e2| ActorError::StartupFailed(e2.to_string()))?
                } else {
                    return Err(ActorError::StartupFailed(reason));
                }
            }
        };

        schema::apply_schema(
            db.db()
                .map_err(|e| ActorError::StartupFailed(e.to_string()))?,
        )
        .map_err(|e| ActorError::StartupFailed(e.to_string()))?;

        // Recover next_event_id from the database.
        {
            let redb = db
                .db()
                .map_err(|e| ActorError::StartupFailed(e.to_string()))?;
            if let Ok(read_txn) = redb.begin_read() {
                if let Ok(log) = read_txn.open_table(EVENT_LOG) {
                    if let Some(Ok((k, _))) = log
                        .iter()
                        .ok()
                        .and_then(|mut it: redb::Range<'_, u64, &[u8]>| it.next_back())
                    {
                        self.next_event_id = k.value() + 1;
                    }
                }
            }
        }

        self.db = Some(db);
        self.broadcast_rx = Some(bus.subscribe_broadcast());

        let (tx, rx) = mpsc::channel(SOUL_CHANNEL_CAPACITY);
        bus.register_directed(SOUL_ACTOR_NAME, tx)
            .map_err(|e| ActorError::StartupFailed(e.to_string()))?;
        self.directed_rx = Some(rx);

        bus.broadcast(Event::System(SystemEvent::ActorReady {
            actor_name: "Soul",
        }))
        .await
        .map_err(|e| ActorError::StartupFailed(format!("broadcast ActorReady failed: {}", e)))?;

        self.bus = Some(bus);
        Ok(())
    }

    async fn run(&mut self) -> Result<(), ActorError> {
        let bus = self
            .bus
            .as_ref()
            .ok_or_else(|| ActorError::RuntimeError("bus not set".into()))?
            .clone();

        let mut directed_rx = self
            .directed_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("directed_rx not set".into()))?;

        let mut broadcast_rx = self
            .broadcast_rx
            .take()
            .ok_or_else(|| ActorError::RuntimeError("broadcast_rx not set".into()))?;

        // Periodic wakeup to ensure we can always check for shutdown
        let mut wakeup = tokio::time::interval(std::time::Duration::from_millis(50));
        wakeup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        wakeup.tick().await; // Consume first immediate tick

        loop {
            tokio::select! {
                biased;

                bcast = broadcast_rx.recv() => {
                    match bcast {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => {
                            break;
                        }
                        Ok(Event::System(system_event)) => {
                            let is_boot_complete = matches!(system_event, SystemEvent::BootComplete);
                            let _ = self.absorb_system_signal(system_event);
                            if is_boot_complete {
                                let _ = self.emit_personality_updated(&bus);
                            }
                        }
                        Ok(Event::Platform(platform_event)) => {
                            let _ = self.absorb_platform_signal(platform_event);
                        }
                        Ok(Event::CTP(ctp_event)) => {
                            let _ = self.absorb_ctp_signal(ctp_event);
                        }
                        Ok(Event::Inference(inference_event)) => {
                            let _ = self.absorb_inference_signal(inference_event);
                        }
                        Ok(Event::Memory(MemoryEvent::ConsolidationCompleted(_))) => {
                            let _ = self.increment_identity_counter(
                                "memory::consolidation_completed_count",
                                1,
                            );
                        }
                        Ok(Event::Soul(SoulEvent::ReadRequested(req))) => {
                            let _ = self.handle_read(req, &bus);
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                    }
                }
                _ = wakeup.tick() => {
                    // Periodic wakeup to ensure shutdown responsiveness
                }
                msg = directed_rx.recv() => {
                    match msg {
                        Some(Event::System(SystemEvent::ShutdownSignal)) => {
                            break;
                        }
                        Some(Event::Soul(soul_event)) => {
                            match soul_event {
                                SoulEvent::WriteRequested(req) => {
                                    let _ = self.handle_write(req, &bus);
                                }
                                SoulEvent::SummaryRequested(req) => {
                                    let _ = self.handle_summary(req, &bus);
                                }
                                SoulEvent::IdentitySignalEmitted(signal) => {
                                    let _ = self.handle_identity_signal(signal.key, signal.value);
                                    let _ = self.emit_personality_updated(&bus);
                                }
                                SoulEvent::InitializeWithName { name } => {
                                    let _ = self.handle_initialize_with_name(name, &bus);
                                }
                                SoulEvent::ExportRequested { path } => {
                                    // TODO M6: implement full export (event log + identity signals → JSON).
                                    let _ = bus
                                        .broadcast(Event::Soul(SoulEvent::ExportFailed {
                                            reason: "Soul export not yet implemented (M6)".to_string(),
                                        }))
                                        .await;
                                    let _ = path; // suppress unused warning until M6
                                }
                                _ => {}
                            }
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        // Drain outstanding tasks with a 1-second timeout.
        // Tasks that don't complete (e.g., blocked on broadcast sends during shutdown)
        // are aborted to prevent actor stop timeout.
        let drain_timeout = Duration::from_secs(1);
        let drain_deadline = tokio::time::Instant::now() + drain_timeout;

        loop {
            let remaining = drain_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                // Timeout reached — abort all remaining tasks
                self.task_set.abort_all();
                break;
            }

            match tokio::time::timeout(remaining, self.task_set.join_next()).await {
                Ok(Some(_)) => continue, // Task completed, keep draining
                Ok(None) => break,       // All tasks completed
                Err(_) => {
                    // Timeout reached — abort remaining
                    self.task_set.abort_all();
                    break;
                }
            }
        }

        if let Some(db) = self.db.take() {
            db.close().map_err(|e| {
                ActorError::RuntimeError(format!("failed to close encrypted db: {e}"))
            })?;
        }
        self.bus = None;
        self.directed_rx = None;
        self.broadcast_rx = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::events::soul::{SoulSummaryRequested, SoulWriteRequest};
    use std::time::SystemTime;
    use tempfile::tempdir;

    fn test_key() -> crypto::MasterKey {
        crypto::MasterKey::from_bytes([42u8; 32])
    }

    #[tokio::test]
    async fn soul_actor_lifecycle_starts_and_stops() {
        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");
        actor.stop().await.expect("stop should succeed");

        assert!(db_path.exists());
    }

    #[tokio::test]
    async fn soul_actor_write_and_summary() {
        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume the ActorReady event emitted during start()
        let ready_event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive ActorReady within timeout")
            .expect("should receive event");
        assert!(matches!(
            ready_event,
            Event::System(SystemEvent::ActorReady { actor_name: "Soul" })
        ));

        let req = SoulWriteRequest {
            description: "user opened editor".to_string(),
            app_context: Some("Code".to_string()),
            timestamp: SystemTime::now(),
            request_id: 1,
        };
        actor.handle_write(req, &bus).expect("write should succeed");

        let event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("should receive event");
        assert!(matches!(event, Event::Soul(SoulEvent::EventLogged(_))));

        let summary_req = SoulSummaryRequested {
            max_events: 10,
            request_id: 2,
        };
        actor
            .handle_summary(summary_req, &bus)
            .expect("summary should succeed");

        let summary_event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("should receive event");

        if let Event::Soul(SoulEvent::SummaryReady(s)) = summary_event {
            assert_eq!(s.event_count, 1);
            assert!(s.content.contains("user opened editor"));
        } else {
            panic!("Expected SummaryReady, got {:?}", summary_event);
        }

        actor.stop().await.expect("stop should succeed");
    }

    #[tokio::test]
    async fn soul_initializes_user_name() {
        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume ActorReady
        let _ready = rx.recv().await.expect("should receive ActorReady");

        // Send InitializeWithName
        actor
            .handle_initialize_with_name("Alice".to_string(), &bus)
            .expect("initialize name should succeed");

        // Verify NameInitialized event
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("should receive event");

        if let Event::Soul(SoulEvent::NameInitialized(init)) = event {
            assert_eq!(init.name, "Alice");
        } else {
            panic!("Expected NameInitialized, got {:?}", event);
        }

        // Verify name can be read back from the database
        {
            let db = actor.db.as_ref().expect("db should be initialized");
            let redb = db.db().expect("should get redb");
            let read_txn = redb.begin_read().expect("should begin read");
            let table = read_txn
                .open_table(USER_IDENTITY)
                .expect("should open USER_IDENTITY");
            let name_guard = table
                .get("user_name")
                .expect("should get user_name")
                .expect("user_name should exist");
            let name = name_guard.value();
            assert_eq!(name, "Alice");
        } // Guards dropped here

        actor.stop().await.expect("stop should succeed");
    }

    #[tokio::test]
    async fn soul_rejects_empty_name() {
        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        let result = actor.handle_initialize_with_name("".to_string(), &bus);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid user name"));

        actor.stop().await.expect("stop should succeed");
    }

    #[tokio::test]
    async fn soul_rejects_too_long_name() {
        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        let long_name = "a".repeat(51);
        let result = actor.handle_initialize_with_name(long_name, &bus);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid user name"));

        actor.stop().await.expect("stop should succeed");
    }

    #[tokio::test]
    async fn soul_actor_clean_shutdown_with_concurrent_writes() {
        // Regression test for shutdown timeout issue: ensure spawned broadcast
        // tasks are drained before encrypted database closes.
        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume ActorReady
        let _ = rx.recv().await.expect("ActorReady event");

        // Issue multiple write requests that will spawn broadcast tasks
        for i in 0..5 {
            let req = SoulWriteRequest {
                description: format!("event {}", i),
                app_context: Some("test".to_string()),
                timestamp: SystemTime::now(),
                request_id: i + 1,
            };
            actor.handle_write(req, &bus).expect("write should succeed");
        }

        // Give tasks a moment to spawn
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // stop() should complete without timeout even with pending broadcast tasks
        let stop_result =
            tokio::time::timeout(std::time::Duration::from_secs(2), actor.stop()).await;
        assert!(stop_result.is_ok(), "stop should complete within timeout");
        assert!(
            stop_result.unwrap().is_ok(),
            "stop should succeed after draining tasks"
        );
    }

    #[tokio::test]
    async fn soul_actor_stops_on_directed_shutdown_signal() {
        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Consume ActorReady
        let _ = rx.recv().await.expect("ActorReady event");

        let run_handle = tokio::spawn(async move { actor.run().await });

        bus.send_directed("soul", Event::System(SystemEvent::ShutdownSignal))
            .await
            .expect("directed shutdown should send");

        let run_result = tokio::time::timeout(std::time::Duration::from_secs(2), run_handle)
            .await
            .expect("run loop should stop within timeout")
            .expect("run loop should not panic");
        assert!(run_result.is_ok(), "run loop should complete cleanly");
    }
}
