//! IPC server for CLI ↔ daemon communication.
//!
//! Phase 6: The daemon spawns an IPC server (Unix socket on macOS/Linux, named pipe on Windows)
//! that accepts multiple CLI connections. Each CLI session:
//! 1. Connects to the IPC endpoint
//! 2. Sends IpcMessage::Subscribe to register for event stream
//! 3. Sends slash commands and chat messages
//! 4. Receives DisplayLine and other response payloads from the daemon
//!
//! The server translates IpcPayload commands to bus events and streams relevant bus
//! events back to subscribed clients.

use bus::events::inference::{InferenceEvent, Priority};
use bus::events::soul::SoulEvent;
use bus::events::speech::SpeechEvent;
use bus::events::system::SystemEvent;
use bus::events::transparency::{TransparencyEvent, TransparencyQuery};
use bus::ipc::{IpcMessage, IpcPayload, LineStyle, IPC_SCHEMA_VERSION};
use bus::{Event, EventBus};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

#[cfg(unix)]
use std::os::unix::net::UnixListener as StdUnixListener;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

/// IPC server state.
pub struct IpcServer {
    bus: Arc<EventBus>,
    sessions: Arc<Mutex<HashMap<u64, SessionHandle>>>,
    next_session_id: AtomicU64,
    /// Tracks the current enabled state for every registered background loop.
    /// Initialized to `true` for all 5 canonical loops ({\S}17.2).
    /// Updated whenever `SystemEvent::LoopStatusChanged` fires on the bus.
    /// New clients receive the full current state as `LoopStatusUpdate` bursts on subscribe.
    loop_states: Arc<Mutex<HashMap<String, bool>>>,
}

/// Per-session handle for cleanup.
struct SessionHandle {
    /// Channel to send DisplayLine messages to the client write task.
    #[allow(dead_code)]
    tx: mpsc::UnboundedSender<IpcMessage>,
}

impl IpcServer {
    pub fn new(bus: Arc<EventBus>) -> Self {
        let mut default_states = HashMap::new();
        for name in &[
            "ctp",
            "memory_consolidation",
            "platform_polling",
            "screen_capture",
            "speech",
        ] {
            default_states.insert((*name).to_string(), true);
        }
        Self {
            bus,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_session_id: AtomicU64::new(1),
            loop_states: Arc::new(Mutex::new(default_states)),
        }
    }

    /// Start the IPC server and listen for connections.
    pub async fn start(self: Arc<Self>) -> Result<(), IpcServerError> {
        #[cfg(unix)]
        {
            self.start_unix().await
        }

        #[cfg(windows)]
        {
            self.start_windows().await
        }
    }

    /// Start the IPC server on a custom path (for testing).
    #[cfg(unix)]
    pub async fn start_on(self: Arc<Self>, path: impl AsRef<str>) -> Result<(), IpcServerError> {
        self.start_unix_on(path.as_ref().to_string()).await
    }

    /// Start the IPC server on a custom path (for testing).
    #[cfg(windows)]
    pub async fn start_on(self: Arc<Self>, path: impl AsRef<str>) -> Result<(), IpcServerError> {
        self.start_windows_on(path.as_ref().to_string()).await
    }

    #[cfg(unix)]
    async fn start_unix(self: Arc<Self>) -> Result<(), IpcServerError> {
        let socket_path = ipc_socket_path();
        self.start_unix_on(socket_path.to_string_lossy().to_string())
            .await
    }

    #[cfg(unix)]
    async fn start_unix_on(self: Arc<Self>, socket_path: String) -> Result<(), IpcServerError> {
        // Remove stale socket file if it exists.
        let _ = std::fs::remove_file(&socket_path);

        // Bind the Unix socket.
        let std_listener = StdUnixListener::bind(&socket_path)
            .map_err(|e| IpcServerError::BindFailed(e.to_string()))?;
        std_listener
            .set_nonblocking(true)
            .map_err(|e| IpcServerError::BindFailed(e.to_string()))?;

        let listener = UnixListener::from_std(std_listener)
            .map_err(|e| IpcServerError::BindFailed(e.to_string()))?;

        tracing::info!("IPC server listening on {:?}", socket_path);

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let session_id = self.next_session_id.fetch_add(1, Ordering::SeqCst);
                    tracing::info!("IPC client connected: session_id={}", session_id);

                    let server = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client_unix(stream, server, session_id).await {
                            tracing::error!("IPC session {} failed: {}", session_id, e);
                        }
                        tracing::info!("IPC session {} disconnected", session_id);
                    });
                }
                Err(e) => {
                    tracing::error!("IPC accept failed: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    #[cfg(windows)]
    async fn start_windows(self: Arc<Self>) -> Result<(), IpcServerError> {
        let pipe_name = r"\\.\pipe\sena_ipc";
        self.start_windows_on(pipe_name.to_string()).await
    }

    #[cfg(windows)]
    async fn start_windows_on(self: Arc<Self>, pipe_name: String) -> Result<(), IpcServerError> {
        tracing::info!("IPC server listening on {}", pipe_name);

        loop {
            let pipe = ServerOptions::new()
                .first_pipe_instance(false)
                .create(&pipe_name)
                .map_err(|e| IpcServerError::BindFailed(e.to_string()))?;

            // Wait for a client to connect.
            pipe.connect()
                .await
                .map_err(|e| IpcServerError::BindFailed(e.to_string()))?;

            let session_id = self.next_session_id.fetch_add(1, Ordering::SeqCst);
            tracing::info!("IPC client connected: session_id={}", session_id);

            let server = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = Self::handle_client_windows(pipe, server, session_id).await {
                    tracing::error!("IPC session {} failed: {}", session_id, e);
                }
                tracing::info!("IPC session {} disconnected", session_id);
            });
        }
    }

    #[cfg(unix)]
    async fn handle_client_unix(
        stream: UnixStream,
        server: Arc<IpcServer>,
        session_id: u64,
    ) -> Result<(), IpcServerError> {
        let (read_half, write_half) = stream.into_split();
        let reader = BufReader::new(read_half);
        let mut lines = reader.lines();

        let (tx, mut rx) = mpsc::unbounded_channel::<IpcMessage>();

        // Register session.
        {
            let mut sessions = server.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.insert(session_id, SessionHandle { tx: tx.clone() });
        }

        // Spawn write task.
        let mut write_half = write_half;
        let write_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let json = serde_json::to_string(&msg).unwrap_or_default();
                let line = format!("{}\n", json);
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        // Spawn bus event push task (subscribes to broadcast).
        let bus_tx = tx.clone();
        let bus = Arc::clone(&server.bus);
        let loop_states_bus = Arc::clone(&server.loop_states);
        let bus_task = tokio::spawn(async move {
            let mut bus_rx = bus.subscribe_broadcast();
            while let Ok(event) = bus_rx.recv().await {
                // Forward loop state changes to this client and update server-side registry.
                if let Event::System(SystemEvent::LoopStatusChanged {
                    ref loop_name,
                    enabled,
                }) = event
                {
                    {
                        let mut states = loop_states_bus.lock().unwrap_or_else(|e| e.into_inner());
                        states.insert(loop_name.clone(), enabled);
                    }
                    let _ = bus_tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::LoopStatusUpdate {
                            loop_name: loop_name.clone(),
                            enabled,
                        },
                    });
                }
                if let Event::Inference(InferenceEvent::ModelLoaded { ref name, .. }) = event {
                    let _ = bus_tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::ModelStatusUpdate { name: name.clone() },
                    });
                }
                if let Some(display_msg) = event_to_display_line(&event) {
                    let _ = bus_tx.send(display_msg);
                }
                if matches!(event, Event::System(SystemEvent::ShutdownSignal)) {
                    let _ = bus_tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::DaemonShutdown,
                    });
                    break;
                }
            }
        });

        // Read loop: process incoming messages.
        while let Ok(Some(line)) = lines.next_line().await {
            let msg: IpcMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("IPC session {}: malformed JSON: {}", session_id, e);
                    continue;
                }
            };

            server.handle_message(msg, &tx).await;
        }

        // Cleanup: remove session, cancel tasks.
        {
            let mut sessions = server.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.remove(&session_id);
        }
        write_task.abort();
        bus_task.abort();

        Ok(())
    }

    #[cfg(windows)]
    async fn handle_client_windows(
        pipe: NamedPipeServer,
        server: Arc<IpcServer>,
        session_id: u64,
    ) -> Result<(), IpcServerError> {
        let (read_half, write_half) = tokio::io::split(pipe);
        let reader = BufReader::new(read_half);
        let mut lines = reader.lines();

        let (tx, mut rx) = mpsc::unbounded_channel::<IpcMessage>();

        // Register session.
        {
            let mut sessions = server.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.insert(session_id, SessionHandle { tx: tx.clone() });
        }

        // Spawn write task.
        let mut write_half = write_half;
        let write_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let json = serde_json::to_string(&msg).unwrap_or_default();
                let line = format!("{}\n", json);
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        // Spawn bus event push task.
        let bus_tx = tx.clone();
        let bus = Arc::clone(&server.bus);
        let loop_states_bus = Arc::clone(&server.loop_states);
        let bus_task = tokio::spawn(async move {
            let mut bus_rx = bus.subscribe_broadcast();
            while let Ok(event) = bus_rx.recv().await {
                // Forward loop state changes to this client and update server-side registry.
                if let Event::System(SystemEvent::LoopStatusChanged {
                    ref loop_name,
                    enabled,
                }) = event
                {
                    {
                        let mut states = loop_states_bus.lock().unwrap_or_else(|e| e.into_inner());
                        states.insert(loop_name.clone(), enabled);
                    }
                    let _ = bus_tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::LoopStatusUpdate {
                            loop_name: loop_name.clone(),
                            enabled,
                        },
                    });
                }
                if let Event::Inference(InferenceEvent::ModelLoaded { ref name, .. }) = event {
                    let _ = bus_tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::ModelStatusUpdate { name: name.clone() },
                    });
                }
                if let Some(display_msg) = event_to_display_line(&event) {
                    let _ = bus_tx.send(display_msg);
                }
                if matches!(event, Event::System(SystemEvent::ShutdownSignal)) {
                    let _ = bus_tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::DaemonShutdown,
                    });
                    break;
                }
            }
        });

        // Read loop.
        while let Ok(Some(line)) = lines.next_line().await {
            let msg: IpcMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("IPC session {}: malformed JSON: {}", session_id, e);
                    continue;
                }
            };

            server.handle_message(msg, &tx).await;
        }

        // Cleanup.
        {
            let mut sessions = server.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions.remove(&session_id);
        }
        write_task.abort();
        bus_task.abort();

        Ok(())
    }

    /// Handle an incoming IpcMessage from a client.
    async fn handle_message(&self, msg: IpcMessage, tx: &mpsc::UnboundedSender<IpcMessage>) {
        match msg.payload {
            IpcPayload::Subscribe => {
                tracing::info!("IPC client subscribed");
                // Read the preferred model from config so the CLI sidebar shows it immediately.
                let current_model = crate::config::load_or_create_config()
                    .await
                    .ok()
                    .and_then(|c| c.preferred_model);
                let _ = tx.send(IpcMessage {
                    id: msg.id,
                    payload: IpcPayload::SessionReady {
                        schema_version: IPC_SCHEMA_VERSION,
                        current_model,
                    },
                });
                // Send initial loop state burst — one LoopStatusUpdate per registered loop.
                // This ensures the CLI sidebar is correct immediately on connection.
                let states = self
                    .loop_states
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                for (loop_name, enabled) in states {
                    let _ = tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::LoopStatusUpdate { loop_name, enabled },
                    });
                }
            }
            IpcPayload::Chat { text } => {
                let request_id = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(1);

                if let Err(e) = self
                    .bus
                    .send_directed(
                        "inference",
                        Event::Inference(InferenceEvent::InferenceRequested {
                            prompt: text,
                            priority: Priority::High,
                            request_id,
                            source: bus::InferenceSource::UserText,
                        }),
                    )
                    .await
                {
                    let _ = tx.send(IpcMessage {
                        id: msg.id,
                        payload: IpcPayload::Error {
                            to_id: msg.id,
                            reason: format!("Failed to dispatch chat: {}", e),
                        },
                    });
                } else {
                    let _ = tx.send(IpcMessage {
                        id: msg.id,
                        payload: IpcPayload::Ack { to_id: msg.id },
                    });
                }
            }
            IpcPayload::SlashCommand { line } => {
                let outputs = dispatch_slash_command(&line, &self.bus, &self.loop_states).await;
                for (content, style) in outputs {
                    let _ = tx.send(IpcMessage {
                        id: 0,
                        payload: IpcPayload::DisplayLine { content, style },
                    });
                }
                let _ = tx.send(IpcMessage {
                    id: msg.id,
                    payload: IpcPayload::Ack { to_id: msg.id },
                });
            }
            IpcPayload::Ping => {
                let _ = tx.send(IpcMessage {
                    id: msg.id,
                    payload: IpcPayload::Pong,
                });
            }
            IpcPayload::ShutdownRequested => {
                // Only a CLI that auto-started the daemon sends this.
                // Broadcast ShutdownSignal so the runtime shuts down cleanly.
                if let Err(e) = self
                    .bus
                    .broadcast(Event::System(SystemEvent::ShutdownSignal))
                    .await
                {
                    tracing::error!(
                        "IPC ShutdownRequested: failed to broadcast shutdown signal: {}",
                        e
                    );
                } else {
                    tracing::info!("IPC ShutdownRequested: shutdown signal dispatched");
                }
            }
            IpcPayload::InitializeName { name } => {
                // First-boot onboarding: CLI collected user name before daemon started.
                // Broadcast InitializeWithName so Soul can persist it.
                if let Err(e) = self
                    .bus
                    .broadcast(Event::Soul(SoulEvent::InitializeWithName {
                        name: name.clone(),
                    }))
                    .await
                {
                    let _ = tx.send(IpcMessage {
                        id: msg.id,
                        payload: IpcPayload::Error {
                            to_id: msg.id,
                            reason: format!("Failed to initialize name: {}", e),
                        },
                    });
                } else {
                    let _ = tx.send(IpcMessage {
                        id: msg.id,
                        payload: IpcPayload::Ack { to_id: msg.id },
                    });
                }
            }
            _ => {
                let _ = tx.send(IpcMessage {
                    id: msg.id,
                    payload: IpcPayload::Error {
                        to_id: msg.id,
                        reason: "unknown command".to_string(),
                    },
                });
            }
        }
    }
}

/// Dispatch a slash command line to the appropriate handler.
///
/// Returns a list of (content, style) tuples to display to the client.
async fn dispatch_slash_command(
    line: &str,
    bus: &Arc<EventBus>,
    loop_states: &Arc<Mutex<HashMap<String, bool>>>,
) -> Vec<(String, LineStyle)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let cmd = parts.first().copied().unwrap_or("");

    match cmd {
        "/observation" | "/obs" => {
            if let Err(e) = bus
                .broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
                    TransparencyQuery::CurrentObservation,
                )))
                .await
            {
                vec![(
                    format!("Failed to query observation: {}", e),
                    LineStyle::Error,
                )]
            } else {
                vec![(
                    "Querying observation...".to_string(),
                    LineStyle::SystemNotice,
                )]
            }
        }
        "/memory" | "/mem" => {
            if let Err(e) = bus
                .broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
                    TransparencyQuery::UserMemory,
                )))
                .await
            {
                vec![(format!("Failed to query memory: {}", e), LineStyle::Error)]
            } else {
                vec![("Querying memory...".to_string(), LineStyle::SystemNotice)]
            }
        }
        "/explanation" | "/why" => {
            if let Err(e) = bus
                .broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
                    TransparencyQuery::InferenceExplanation,
                )))
                .await
            {
                vec![(
                    format!("Failed to query explanation: {}", e),
                    LineStyle::Error,
                )]
            } else {
                vec![(
                    "Querying last inference...".to_string(),
                    LineStyle::SystemNotice,
                )]
            }
        }
        "/config" => {
            if parts.get(1) == Some(&"set") {
                let key = parts.get(2).copied().unwrap_or("");
                let value = if parts.len() > 3 {
                    parts[3..].join(" ")
                } else {
                    String::new()
                };

                if let Err(e) = bus
                    .broadcast(Event::System(SystemEvent::ConfigSetRequested {
                        key: key.to_string(),
                        value: value.clone(),
                    }))
                    .await
                {
                    vec![(format!("Config set failed: {}", e), LineStyle::Error)]
                } else {
                    vec![(
                        format!("Setting {} = {}...", key, value),
                        LineStyle::SystemNotice,
                    )]
                }
            } else if parts.get(1) == Some(&"reload") {
                if let Err(e) = bus
                    .broadcast(Event::System(SystemEvent::ConfigReloadRequested))
                    .await
                {
                    vec![(format!("Config reload failed: {}", e), LineStyle::Error)]
                } else {
                    vec![(
                        "Reloading config from disk...".to_string(),
                        LineStyle::SystemNotice,
                    )]
                }
            } else {
                match crate::config::load_or_create_config().await {
                    Ok(config) => {
                        let mut lines = vec![];
                        lines.push(("━━  Configuration".to_string(), LineStyle::SystemNotice));

                        if let Ok(path) = crate::config::config_path() {
                            lines.push((
                                format!("Config file: {}", path.display()),
                                LineStyle::Normal,
                            ));
                        }

                        match toml::to_string_pretty(&config) {
                            Ok(toml_str) => {
                                for line in toml_str.lines() {
                                    lines.push((line.to_string(), LineStyle::Normal));
                                }
                            }
                            Err(e) => {
                                lines.push((
                                    format!("Could not serialize config: {}", e),
                                    LineStyle::Error,
                                ));
                            }
                        }

                        lines.push(("".to_string(), LineStyle::Normal));
                        lines.push((
                            "Use /config set <key> <value> to edit.".to_string(),
                            LineStyle::SystemNotice,
                        ));
                        lines
                    }
                    Err(e) => {
                        vec![(format!("Failed to load config: {}", e), LineStyle::Error)]
                    }
                }
            }
        }
        "/reload" => {
            if let Err(e) = bus
                .broadcast(Event::System(SystemEvent::ConfigReloadRequested))
                .await
            {
                vec![(format!("Config reload failed: {}", e), LineStyle::Error)]
            } else {
                vec![(
                    "Reloading config from disk...".to_string(),
                    LineStyle::SystemNotice,
                )]
            }
        }
        "/actors" => {
            // All actors are confirmed Ready by the time IPC server starts (boot_ready waits
            // for ActorReady from every actor). ActorFailed events are broadcast on the bus,
            // but that handling is not yet wired in Phase 6.1. For now, return static "all ready".
            vec![
                ("━━  Actor Status".to_string(), LineStyle::SystemNotice),
                ("Platform     ✓ Ready".to_string(), LineStyle::Normal),
                ("Inference    ✓ Ready".to_string(), LineStyle::Normal),
                ("CTP          ✓ Ready".to_string(), LineStyle::Normal),
                ("Memory       ✓ Ready".to_string(), LineStyle::Normal),
                ("Soul         ✓ Ready".to_string(), LineStyle::Normal),
                ("".to_string(), LineStyle::Normal),
                (
                    "All actors are running. Use /shutdown to stop the daemon.".to_string(),
                    LineStyle::SystemNotice,
                ),
            ]
        }
        "/models" => {
            if let Err(e) = bus
                .broadcast(Event::Transparency(TransparencyEvent::QueryRequested(
                    TransparencyQuery::ModelList,
                )))
                .await
            {
                vec![(format!("Failed to query models: {}", e), LineStyle::Error)]
            } else {
                vec![(
                    "Querying model registry...".to_string(),
                    LineStyle::SystemNotice,
                )]
            }
        }
        "/voice" => {
            vec![(
                "Voice toggle is CLI-local — not applicable in IPC mode.".to_string(),
                LineStyle::SystemNotice,
            )]
        }
        "/speech" => match crate::config::load_or_create_config().await {
            Ok(config) => {
                vec![
                    (
                        "\u{2501}\u{2501}  Speech Configuration".to_string(),
                        LineStyle::SystemNotice,
                    ),
                    (
                        format!("speech_enabled: {}", config.speech_enabled),
                        LineStyle::Normal,
                    ),
                    (
                        format!("voice_always_listening: {}", config.voice_always_listening),
                        LineStyle::Normal,
                    ),
                    (
                        format!("wakeword_enabled: {}", config.wakeword_enabled),
                        LineStyle::Normal,
                    ),
                    (
                        format!("tts_rate: {:.1}", config.tts_rate),
                        LineStyle::Normal,
                    ),
                    (
                        format!(
                            "proactive_speech_enabled: {}",
                            config.proactive_speech_enabled
                        ),
                        LineStyle::Normal,
                    ),
                ]
            }
            Err(e) => vec![(format!("Failed to load config: {}", e), LineStyle::Error)],
        },
        "/listen" => {
            // Subcommand format (sent by CLI): "/listen start <session_id>" or "/listen stop <session_id>"
            match parts.get(1).copied() {
                Some("start") => {
                    let session_id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    if let Err(e) = bus
                        .broadcast(Event::Speech(SpeechEvent::ListenModeRequested {
                            session_id,
                        }))
                        .await
                    {
                        vec![(
                            format!("Failed to start listen mode: {}", e),
                            LineStyle::Error,
                        )]
                    } else {
                        vec![(
                            "\u{1f3a4} Listen mode started — type /listen again to stop."
                                .to_string(),
                            LineStyle::SystemNotice,
                        )]
                    }
                }
                Some("stop") => {
                    let session_id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    if let Err(e) = bus
                        .broadcast(Event::Speech(SpeechEvent::ListenModeStopRequested {
                            session_id,
                        }))
                        .await
                    {
                        vec![(
                            format!("Failed to stop listen mode: {}", e),
                            LineStyle::Error,
                        )]
                    } else {
                        vec![(
                            "\u{1f3a4} Stopping listen mode...".to_string(),
                            LineStyle::SystemNotice,
                        )]
                    }
                }
                _ => {
                    vec![(
                        "Usage: /listen (toggled by CLI — use /listen from the TUI).".to_string(),
                        LineStyle::SystemNotice,
                    )]
                }
            }
        }
        "/microphone" => {
            vec![(
                "Microphone selection not yet implemented in IPC mode.".to_string(),
                LineStyle::SystemNotice,
            )]
        }
        "/screenshot" => {
            vec![(
                "Screenshot status not yet implemented in IPC mode.".to_string(),
                LineStyle::SystemNotice,
            )]
        }
        "/verbose" => {
            vec![(
                "Verbose mode is CLI-local — not applicable in IPC mode.".to_string(),
                LineStyle::SystemNotice,
            )]
        }
        "/copy" => {
            vec![(
                "Clipboard copy is CLI-local — not applicable in IPC mode.".to_string(),
                LineStyle::SystemNotice,
            )]
        }
        "/help" | "/h" => {
            vec![
                ("━━  Commands".to_string(), LineStyle::SystemNotice),
                (
                    "/observation or /obs   — What are you observing right now?".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/memory or /mem        — What do you remember about me?".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/explanation or /why   — Why did you say that?".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/config                — Show and edit settings".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/reload                — Reload config from disk".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/actors                — Show actor health status".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/loops                 — Show and toggle background loops".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/speech                — View speech configuration".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/help                  — Show this message".to_string(),
                    LineStyle::Normal,
                ),
                (
                    "/shutdown              — Shut down Sena completely".to_string(),
                    LineStyle::Normal,
                ),
            ]
        }
        "/close" | "/quit" | "/exit" | "/q" => {
            vec![(
                "To close CLI, disconnect. Daemon keeps running.".to_string(),
                LineStyle::SystemNotice,
            )]
        }
        "/shutdown" => {
            if let Err(e) = bus
                .broadcast(Event::System(SystemEvent::ShutdownSignal))
                .await
            {
                vec![(
                    format!("Failed to send shutdown signal: {}", e),
                    LineStyle::Error,
                )]
            } else {
                vec![(
                    "Shutdown signal sent — daemon shutting down.".to_string(),
                    LineStyle::SystemNotice,
                )]
            }
        }
        "/loops" => {
            let args: Vec<&str> = parts.iter().skip(1).copied().collect();
            match args.as_slice() {
                [] => {
                    // List all loops with current state.
                    let states = loop_states
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    let loop_order: &[(&str, &str)] = &[
                        ("ctp", "CTP"),
                        ("memory_consolidation", "Memory Consolidation"),
                        ("platform_polling", "Platform Polling"),
                        ("screen_capture", "Screen Capture"),
                        ("speech", "Speech"),
                    ];
                    let mut lines = vec![(
                        "\u{2501}\u{2501}  Background Loops".to_string(),
                        LineStyle::SystemNotice,
                    )];
                    for (name, label) in loop_order {
                        let enabled = states.get(*name).copied().unwrap_or(true);
                        // The dot color is rendered by the CLI sidebar; here we use text status.
                        let status = if enabled { "enabled" } else { "disabled" };
                        lines.push((format!("  {} — {}", label, status), LineStyle::Normal));
                    }
                    lines.push(("".to_string(), LineStyle::Normal));
                    lines.push((
                        "Use /loops <name> to toggle. /loops <name> on|off to set.".to_string(),
                        LineStyle::SystemNotice,
                    ));
                    lines
                }
                [name] => {
                    let name = name.to_lowercase();
                    if !is_valid_loop_name(&name) {
                        return vec![(
                            format!(
                                "Unknown loop '{name}'. Valid: ctp, memory_consolidation, platform_polling, screen_capture, speech"
                            ),
                            LineStyle::Error,
                        )];
                    }
                    let current = loop_states
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(name.as_str())
                        .copied()
                        .unwrap_or(true);
                    let new_state = !current;
                    if let Err(e) = bus
                        .broadcast(Event::System(SystemEvent::LoopControlRequested {
                            loop_name: name.clone(),
                            enabled: new_state,
                        }))
                        .await
                    {
                        return vec![(
                            format!("Failed to send loop control: {e}"),
                            LineStyle::Error,
                        )];
                    }
                    let action = if new_state { "Enabling" } else { "Disabling" };
                    vec![(
                        format!("{action} loop '{name}'..."),
                        LineStyle::SystemNotice,
                    )]
                }
                [name, state] => {
                    let name = name.to_lowercase();
                    let state = state.to_lowercase();
                    if !is_valid_loop_name(&name) {
                        return vec![(
                            format!(
                                "Unknown loop '{name}'. Valid: ctp, memory_consolidation, platform_polling, screen_capture, speech"
                            ),
                            LineStyle::Error,
                        )];
                    }
                    let enabled = match state.as_str() {
                        "on" | "enable" | "true" => true,
                        "off" | "disable" | "false" => false,
                        _ => {
                            return vec![(
                                format!("Invalid state '{state}'. Use on or off."),
                                LineStyle::Error,
                            )]
                        }
                    };
                    if let Err(e) = bus
                        .broadcast(Event::System(SystemEvent::LoopControlRequested {
                            loop_name: name.clone(),
                            enabled,
                        }))
                        .await
                    {
                        return vec![(
                            format!("Failed to send loop control: {e}"),
                            LineStyle::Error,
                        )];
                    }
                    let action = if enabled { "Enabling" } else { "Disabling" };
                    vec![(
                        format!("{action} loop '{name}'..."),
                        LineStyle::SystemNotice,
                    )]
                }
                _ => vec![(
                    "Usage: /loops | /loops <name> | /loops <name> on|off".to_string(),
                    LineStyle::Error,
                )],
            }
        }
        _ if line.starts_with('/') => {
            vec![(
                format!("Unknown command '{}'. Type /help for commands.", cmd),
                LineStyle::Error,
            )]
        }
        _ => {
            vec![(
                "Unknown command. Type /help for commands.".to_string(),
                LineStyle::Error,
            )]
        }
    }
}

/// Returns true if `name` is a canonical registered loop name (§17.2 of copilot-instructions).
fn is_valid_loop_name(name: &str) -> bool {
    matches!(
        name,
        "ctp" | "memory_consolidation" | "platform_polling" | "screen_capture" | "speech"
    )
}

/// Convert a bus event to a DisplayLine message for subscribed clients.
fn event_to_display_line(event: &Event) -> Option<IpcMessage> {
    match event {
        Event::Inference(InferenceEvent::InferenceCompleted { text, .. }) => {
            if text.trim().is_empty() {
                None
            } else {
                Some(IpcMessage {
                    id: 0,
                    payload: IpcPayload::DisplayLine {
                        content: text.clone(),
                        style: LineStyle::Inference,
                    },
                })
            }
        }
        Event::Inference(InferenceEvent::InferenceFailed { reason, .. }) => Some(IpcMessage {
            id: 0,
            payload: IpcPayload::DisplayLine {
                content: format!("Inference failed: {}", reason),
                style: LineStyle::Error,
            },
        }),
        Event::CTP(bus::events::ctp::CTPEvent::ThoughtEventTriggered(_thought)) => {
            // Only show in verbose mode (CLI-managed state).
            None
        }
        Event::Speech(SpeechEvent::ListenModeTranscription {
            text,
            is_final,
            confidence,
            ..
        }) => {
            if *is_final {
                Some(IpcMessage {
                    id: 0,
                    payload: IpcPayload::DisplayLine {
                        content: text.clone(),
                        style: LineStyle::Inference,
                    },
                })
            } else if *confidence >= 0.4 {
                Some(IpcMessage {
                    id: 0,
                    payload: IpcPayload::DisplayLine {
                        content: format!("[\u{2026}] {}", text),
                        style: LineStyle::Dimmed,
                    },
                })
            } else {
                None
            }
        }
        Event::Speech(SpeechEvent::ListenModeStopped { .. }) => Some(IpcMessage {
            id: 0,
            payload: IpcPayload::DisplayLine {
                content: "\u{1f3a4} Listen mode stopped.".to_string(),
                style: LineStyle::SystemNotice,
            },
        }),
        Event::System(SystemEvent::ConfigReloaded) => Some(IpcMessage {
            id: 0,
            payload: IpcPayload::DisplayLine {
                content: "Config reloaded.".to_string(),
                style: LineStyle::SystemNotice,
            },
        }),
        Event::System(SystemEvent::ConfigSetFailed { key, reason }) => Some(IpcMessage {
            id: 0,
            payload: IpcPayload::DisplayLine {
                content: format!("Config set '{}' failed: {}", key, reason),
                style: LineStyle::Error,
            },
        }),
        Event::Transparency(TransparencyEvent::ObservationResponded(resp)) => {
            let content = format!(
                "━━  Current Observation\n{}",
                format_observation_response(resp)
            );
            Some(IpcMessage {
                id: 0,
                payload: IpcPayload::DisplayLine {
                    content,
                    style: LineStyle::Normal,
                },
            })
        }
        Event::Transparency(TransparencyEvent::MemoryResponded(resp)) => {
            let content = format!("━━  Memory\n{}", format_memory_response(resp));
            Some(IpcMessage {
                id: 0,
                payload: IpcPayload::DisplayLine {
                    content,
                    style: LineStyle::Normal,
                },
            })
        }
        Event::Transparency(TransparencyEvent::InferenceExplanationResponded(resp)) => {
            let content = format!("━━  Last Inference\n{}", format_explanation_response(resp));
            Some(IpcMessage {
                id: 0,
                payload: IpcPayload::DisplayLine {
                    content,
                    style: LineStyle::Normal,
                },
            })
        }
        Event::Transparency(TransparencyEvent::ModelListResponded(resp)) => {
            let content = format!("━━  Available Models\n{}", format_model_list_response(resp));
            Some(IpcMessage {
                id: 0,
                payload: IpcPayload::DisplayLine {
                    content,
                    style: LineStyle::Normal,
                },
            })
        }
        _ => None,
    }
}

fn format_observation_response(resp: &bus::events::transparency::ObservationResponse) -> String {
    let snapshot = &resp.snapshot;
    let app = &snapshot.active_app.app_name;
    let title = snapshot
        .active_app
        .window_title
        .as_deref()
        .unwrap_or("(no title)");
    let task = match &snapshot.inferred_task {
        Some(hint) => format!("{} ({:.0}%)", hint.category, hint.confidence * 100.0),
        None => "(no task inferred)".to_string(),
    };
    let clipboard = if snapshot.clipboard_digest.is_some() {
        "ready"
    } else {
        "empty"
    };
    let rate = snapshot.keystroke_cadence.events_per_minute;
    let secs = snapshot.session_duration.as_secs();
    let session = if secs >= 60 {
        format!("{} min {} sec", secs / 60, secs % 60)
    } else {
        format!("{} sec", secs)
    };
    let mut lines = vec![
        format!("Window     {} \u{2014} {}", app, title),
        format!("Task       {}", task),
        format!("Clipboard  {}", clipboard),
        format!("Keyboard   {:.1} events/min", rate),
        format!("Session    {}", session),
    ];
    if !snapshot.recent_files.is_empty() {
        lines.push(format!(
            "Files      {} recent events",
            snapshot.recent_files.len()
        ));
    }
    if snapshot.visual_context.is_some() {
        lines.push("Screen     captured (vision context ready)".to_string());
    }
    lines.join("\n")
}

fn format_memory_response(resp: &bus::events::transparency::MemoryResponse) -> String {
    let soul = &resp.soul_summary;
    let user_name = soul.user_name.as_deref().unwrap_or("(not set)");
    format!(
        "User: {}\nInference cycles: {}\nMemory chunks: {}\nWork patterns: {}\nTool preferences: {}\nInterests: {}",
        user_name,
        soul.inference_cycle_count,
        resp.memory_chunks.len(),
        soul.work_patterns.join(", "),
        soul.tool_preferences.join(", "),
        soul.interest_clusters.join(", ")
    )
}

fn format_explanation_response(
    resp: &bus::events::transparency::InferenceExplanationResponse,
) -> String {
    format!(
        "Request: {}\nResponse: {} chars\nWorking memory context: {} chunks\nRounds: {}",
        resp.request_context,
        resp.response_text.len(),
        resp.working_memory_context.len(),
        resp.rounds_completed
    )
}

fn format_model_list_response(resp: &bus::events::transparency::ModelListResponse) -> String {
    if resp.models.is_empty() {
        return "No models discovered. Add GGUF models to your configured model directory."
            .to_string();
    }

    let mut lines = vec![];
    let default = resp.default_model.as_deref().unwrap_or("");

    for model in &resp.models {
        let size_gb = model.size_bytes as f64 / 1_000_000_000.0;
        let marker = if model.name == default {
            " (default)"
        } else {
            ""
        };
        lines.push(format!(
            "  {} — {:.1} GB — {:?}{}",
            model.name, size_gb, model.quantization, marker
        ));
    }

    lines.push("".to_string());
    lines.push(format!("Total: {} models", resp.models.len()));

    lines.join("\n")
}

/// IPC socket path — DIFFERENT from single-instance lock path.
#[cfg(unix)]
fn ipc_socket_path() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    std::env::temp_dir().join(format!("sena-ipc-{}.sock", user))
}

#[derive(Debug, thiserror::Error)]
pub enum IpcServerError {
    #[error("bind failed: {0}")]
    BindFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Start the IPC server.
pub async fn start(bus: Arc<EventBus>) -> Result<(), IpcServerError> {
    let server = Arc::new(IpcServer::new(bus));
    server.start().await
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::path::PathBuf;

    #[test]
    fn ipc_server_socket_path_is_distinct_from_lock_path() {
        #[cfg(unix)]
        {
            let lock_path = crate::single_instance::ipc_socket_path();
            let ipc_path = super::ipc_socket_path();
            assert_ne!(
                lock_path, ipc_path,
                "IPC server socket must be different from single-instance lock socket"
            );
        }

        #[cfg(windows)]
        {
            let lock_pipe = r"\\.\pipe\sena_single_instance";
            let ipc_pipe = r"\\.\pipe\sena_ipc";
            assert_ne!(
                lock_pipe, ipc_pipe,
                "IPC server pipe must be different from single-instance lock pipe"
            );
        }
    }
}
