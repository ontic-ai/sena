//! SoulActor: owns the EncryptedDb and handles all Soul subsystem events.

use std::sync::Arc;

use async_trait::async_trait;
use bus::events::soul::{SoulEventLogged, SoulSummary, SoulSummaryRequested, SoulWriteRequest};
use bus::events::{CTPEvent, InferenceEvent, PlatformEvent};
use bus::{Actor, ActorError, Event, EventBus, SoulEvent, SystemEvent};
use redb::ReadableTable;
use tokio::sync::{broadcast, mpsc};

use crate::{
    encrypted_db::EncryptedDb,
    error::SoulError,
    schema::{self, EVENT_LOG, IDENTITY_SIGNALS},
};

const SOUL_ACTOR_NAME: &str = "soul";
const SOUL_CHANNEL_CAPACITY: usize = 256;

/// Actor that owns the SoulBox encrypted database.
///
/// Accepts directed Soul events (WriteRequested, SummaryRequested,
/// IdentitySignalEmitted). Broadcasts EventLogged and SummaryReady back.
pub struct SoulActor {
    db_path: std::path::PathBuf,
    /// Option so we can take() and zero it in start() once EncryptedDb copies it.
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

        // Reverse back to chronological order (oldest first).
        entries.reverse();

        // Append compact identity signal state.
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
            let next_value = next.to_string();
            signals.insert(key, next_value.as_str())?;
        }
        write_txn.commit()?;
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

        let db = EncryptedDb::open(&self.db_path, &master_key)
            .map_err(|e| ActorError::StartupFailed(e.to_string()))?;
        // master_key is now dropped and zeroed (ZeroizeOnDrop).

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
                                _ => {} // EventLogged / SummaryReady are outbound only
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
                        Ok(Event::Inference(inf_event)) => {
                            if let Err(e) = self.absorb_inference_signal(inf_event) {
                                eprintln!("[soul] inference identity extraction failed: {}", e);
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ActorError> {
        if let Some(db) = self.db.take() {
            db.close()
                .map_err(|e| ActorError::RuntimeError(e.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        // Encrypted file should exist
        assert!(db_path.exists());
    }

    #[tokio::test]
    async fn soul_actor_write_and_summary() {
        use bus::events::soul::{SoulSummaryRequested, SoulWriteRequest};
        use std::time::SystemTime;

        let dir = tempdir().expect("should create tempdir");
        let db_path = dir.path().join("soul.redb.enc");

        let mut actor = SoulActor::new(&db_path, test_key());
        let bus = Arc::new(EventBus::new());
        let mut rx = bus.subscribe_broadcast();

        actor
            .start(Arc::clone(&bus))
            .await
            .expect("start should succeed");

        // Write an event
        let req = SoulWriteRequest {
            description: "user opened editor".to_string(),
            app_context: Some("Code".to_string()),
            timestamp: SystemTime::now(),
            request_id: 1,
        };
        actor.handle_write(req, &bus).expect("write should succeed");

        // Flush EventLogged broadcast
        let event = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("should receive event");
        assert!(matches!(event, Event::Soul(SoulEvent::EventLogged(_))));

        // Request a summary
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
}
