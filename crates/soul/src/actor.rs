//! SoulActor: owns the EncryptedDb and handles all Soul subsystem events.

use std::sync::Arc;

use async_trait::async_trait;
use bus::events::soul::{
    SoulEventLogged, SoulReadCompleted, SoulSummary, SoulSummaryRequested, SoulWriteRequest,
};
use bus::events::transparency::SoulSummaryForTransparency;
use bus::events::{CTPEvent, InferenceEvent, PlatformEvent};
use bus::{Actor, ActorError, Event, EventBus, SoulEvent, SystemEvent};
use redb::ReadableTable;
use tokio::sync::{broadcast, mpsc};

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
        tokio::spawn(async move {
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
        &self,
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
        tokio::spawn(async move {
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
            // Store timestamp using SystemTime
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            table.insert("created_at", &*timestamp.to_string())?;
        }
        write_txn.commit()?;

        // Emit NameInitialized ack on bus
        let bus_clone = Arc::clone(bus);
        let name_clone = name.clone();
        tokio::spawn(async move {
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
        if let CTPEvent::ContextSnapshotReady(snapshot) = event {
            if let Some(task) = snapshot.inferred_task {
                let key = format!("task_pref::{}", task.category.to_lowercase());
                self.increment_identity_counter(&key, 1)?;
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

    /// Handle a transparency ReadRequested — build a redacted summary and broadcast ReadCompleted.
    fn handle_read(
        &self,
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
        tokio::spawn(async move {
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
        // delete the stale file and start fresh so the actor stays alive.
        let db = match EncryptedDb::open(&self.db_path, &master_key) {
            Ok(db) => db,
            Err(e) => {
                let reason = e.to_string();
                if reason.contains("decryption failed") || reason.contains("encryption error") {
                    eprintln!(
                        "[soul] WARNING: Decryption failed (master key changed). \
                         Resetting Soul DB — previous history cleared."
                    );
                    if self.db_path.exists() {
                        std::fs::remove_file(&self.db_path).map_err(|io| {
                            ActorError::StartupFailed(format!(
                                "could not remove stale soul db: {io}"
                            ))
                        })?;
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

        loop {
            tokio::select! {
                msg = directed_rx.recv() => {
                    match msg {
                        Some(Event::Soul(soul_event)) => {
                            match soul_event {
                                SoulEvent::WriteRequested(req) => {
                                    if let Err(e) = self.handle_write(req, &bus) {
                                        eprintln!("[soul] write failed: {}", e);
                                    }
                                }
                                SoulEvent::SummaryRequested(req) => {
                                    if let Err(e) = self.handle_summary(req, &bus) {
                                        eprintln!("[soul] summary failed: {}", e);
                                    }
                                }
                                SoulEvent::IdentitySignalEmitted(signal) => {
                                    if let Err(e) = self.handle_identity_signal(signal.key, signal.value) {
                                        eprintln!("[soul] identity signal failed: {}", e);
                                    }
                                }
                                SoulEvent::InitializeWithName { name } => {
                                    if let Err(e) = self.handle_initialize_with_name(name, &bus) {
                                        eprintln!("[soul] failed to initialize user name: {}", e);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
                bcast = broadcast_rx.recv() => {
                    match bcast {
                        Ok(Event::System(SystemEvent::ShutdownSignal)) => break,
                        Ok(Event::Platform(platform_event)) => {
                            if let Err(e) = self.absorb_platform_signal(platform_event) {
                                eprintln!("[soul] platform identity extraction failed: {}", e);
                            }
                        }
                        Ok(Event::CTP(ctp_event)) => {
                            if let Err(e) = self.absorb_ctp_signal(ctp_event) {
                                eprintln!("[soul] ctp identity extraction failed: {}", e);
                            }
                        }
                        Ok(Event::Inference(inference_event)) => {
                            if let Err(e) = self.absorb_inference_signal(inference_event) {
                                eprintln!("[soul] inference identity extraction failed: {}", e);
                            }
                        }
                        Ok(Event::Soul(SoulEvent::ReadRequested(req))) => {
                            if let Err(e) = self.handle_read(req, &bus) {
                                eprintln!("[soul] read failed: {}", e);
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("[soul] broadcast lagged by {} events", n);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        if let Some(db) = self.db.take() {
            if let Err(e) = db.close() {
                eprintln!("[soul] close failed: {}", e);
            }
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
}
