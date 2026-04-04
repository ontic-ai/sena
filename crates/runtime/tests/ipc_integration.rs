//! Integration tests for IPC server.
//!
//! These tests verify the full IPC server stack:
//! - Unix socket creation and cleanup (Unix)
//! - Named pipe creation (Windows)
//! - JSON-over-newline protocol
//! - Subscribe → SessionReady handshake
//! - Ping/Pong keepalive
//! - Slash command dispatch and DisplayLine responses
//! - Multiple client support
//! - Server resilience to client disconnect

use bus::ipc::{IpcMessage, IpcPayload};
use bus::EventBus;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::{timeout, Duration};

#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;

/// Generate a unique test socket path for Unix.
#[cfg(unix)]
fn test_socket_path() -> String {
    format!(
        "/tmp/sena-test-ipc-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Generate a unique test pipe name for Windows.
#[cfg(windows)]
fn test_pipe_name() -> String {
    format!(
        r"\\.\pipe\sena_test_ipc_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// Start the IPC server on a test path and return the path.
async fn start_test_server() -> (String, Arc<EventBus>) {
    #[cfg(unix)]
    let path = test_socket_path();
    #[cfg(windows)]
    let path = test_pipe_name();

    let bus = Arc::new(EventBus::new());
    let server = Arc::new(runtime::ipc_server::IpcServer::new(Arc::clone(&bus)));

    let server_clone = Arc::clone(&server);
    let path_clone = path.clone();
    tokio::spawn(async move {
        let _ = server_clone.start_on(&path_clone).await;
    });

    // Give server more time to bind (Windows pipes need longer).
    tokio::time::sleep(Duration::from_millis(300)).await;

    (path, bus)
}

/// Test client helper for IPC integration tests.
struct TestClient {
    #[cfg(unix)]
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    #[cfg(unix)]
    writer: tokio::net::unix::OwnedWriteHalf,

    #[cfg(windows)]
    reader: BufReader<tokio::io::ReadHalf<tokio::net::windows::named_pipe::NamedPipeClient>>,
    #[cfg(windows)]
    writer: tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,

    next_id: u64,
}

impl TestClient {
    /// Connect to the IPC server at the given path.
    async fn connect(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        #[cfg(unix)]
        {
            // Retry connection on Unix (server might still be binding).
            for attempt in 0..10 {
                match UnixStream::connect(path).await {
                    Ok(stream) => {
                        let (read_half, write_half) = stream.into_split();
                        let reader = BufReader::new(read_half);
                        return Ok(Self {
                            reader,
                            writer: write_half,
                            next_id: 1,
                        });
                    }
                    Err(_) if attempt < 9 => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(e) => return Err(Box::new(e)),
                }
            }
            unreachable!()
        }

        #[cfg(windows)]
        {
            // Windows named pipes: retry connection with wait.
            for attempt in 0..20 {
                match ClientOptions::new().open(path) {
                    Ok(client) => {
                        let (read_half, write_half) = tokio::io::split(client);
                        let reader = BufReader::new(read_half);
                        return Ok(Self {
                            reader,
                            writer: write_half,
                            next_id: 1,
                        });
                    }
                    Err(_) if attempt < 19 => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(e) => return Err(Box::new(e)),
                }
            }
            unreachable!()
        }
    }

    /// Send an IPC message and return the message ID.
    async fn send_msg(&mut self, payload: IpcPayload) -> Result<u64, Box<dyn std::error::Error>> {
        let id = self.next_id;
        self.next_id += 1;

        let msg = IpcMessage { id, payload };
        let json = serde_json::to_string(&msg)?;
        let line = format!("{}\n", json);
        self.writer.write_all(line.as_bytes()).await?;
        Ok(id)
    }

    /// Receive the next IPC message (with 2-second timeout).
    async fn recv_msg(&mut self) -> Option<IpcMessage> {
        let mut line = String::new();
        match timeout(Duration::from_secs(2), self.reader.read_line(&mut line)).await {
            Ok(Ok(n)) if n > 0 => serde_json::from_str(&line).ok(),
            _ => None,
        }
    }

    /// Send Subscribe and wait for SessionReady.
    async fn subscribe_and_wait_ready(&mut self) -> bool {
        if self.send_msg(IpcPayload::Subscribe).await.is_err() {
            return false;
        }

        match self.recv_msg().await {
            Some(msg) => matches!(msg.payload, IpcPayload::SessionReady),
            None => false,
        }
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        // Connection cleanup handled by Drop.
    }
}

/// Cleanup socket file on Unix.
#[cfg(unix)]
fn cleanup_socket(path: &str) {
    let _ = std::fs::remove_file(path);
}

/// No-op cleanup on Windows (named pipes are auto-cleaned).
#[cfg(windows)]
fn cleanup_socket(_path: &str) {
    // Named pipes are cleaned up automatically.
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ipc_client_receives_session_ready_on_connect() {
    let (path, _bus) = start_test_server().await;

    let mut client = TestClient::connect(&path).await.expect("connect failed");

    // Send Subscribe.
    let subscribe_id = client
        .send_msg(IpcPayload::Subscribe)
        .await
        .expect("send Subscribe failed");

    // Receive SessionReady.
    let msg = client.recv_msg().await.expect("no SessionReady received");

    assert_eq!(msg.id, subscribe_id);
    assert!(
        matches!(msg.payload, IpcPayload::SessionReady),
        "Expected SessionReady, got {:?}",
        msg.payload
    );

    cleanup_socket(&path);
}

#[tokio::test]
async fn ipc_ping_gets_pong() {
    let (path, _bus) = start_test_server().await;

    let mut client = TestClient::connect(&path).await.expect("connect failed");

    // Subscribe first.
    assert!(client.subscribe_and_wait_ready().await, "subscribe failed");

    // Send Ping.
    let ping_id = client
        .send_msg(IpcPayload::Ping)
        .await
        .expect("send Ping failed");

    // Receive Pong.
    let msg = client.recv_msg().await.expect("no Pong received");

    assert_eq!(msg.id, ping_id);
    assert!(
        matches!(msg.payload, IpcPayload::Pong),
        "Expected Pong, got {:?}",
        msg.payload
    );

    cleanup_socket(&path);
}

#[tokio::test]
async fn ipc_slash_help_returns_display_lines() {
    let (path, _bus) = start_test_server().await;

    let mut client = TestClient::connect(&path).await.expect("connect failed");

    // Subscribe first.
    assert!(client.subscribe_and_wait_ready().await, "subscribe failed");

    // Send /help.
    let _help_id = client
        .send_msg(IpcPayload::SlashCommand {
            line: "/help".to_string(),
        })
        .await
        .expect("send /help failed");

    // Collect DisplayLine messages. /help produces multiple lines.
    let mut display_lines = vec![];
    let mut ack_received = false;

    for _ in 0..20 {
        // Expect multiple lines + Ack.
        if let Some(msg) = client.recv_msg().await {
            match msg.payload {
                IpcPayload::DisplayLine { .. } => {
                    display_lines.push(msg);
                }
                IpcPayload::Ack { .. } => {
                    ack_received = true;
                    break;
                }
                _ => {}
            }
        } else {
            break;
        }
    }

    assert!(
        !display_lines.is_empty(),
        "/help produced no DisplayLine messages"
    );
    assert!(ack_received, "No Ack received for /help");

    cleanup_socket(&path);
}

#[tokio::test]
async fn ipc_slash_config_returns_display_line() {
    let (path, _bus) = start_test_server().await;

    let mut client = TestClient::connect(&path).await.expect("connect failed");

    // Subscribe first.
    assert!(client.subscribe_and_wait_ready().await, "subscribe failed");

    // Send /config.
    let _config_id = client
        .send_msg(IpcPayload::SlashCommand {
            line: "/config".to_string(),
        })
        .await
        .expect("send /config failed");

    // Collect DisplayLine messages.
    let mut display_lines = vec![];
    let mut ack_received = false;

    for _ in 0..50 {
        // Config can produce many lines.
        if let Some(msg) = client.recv_msg().await {
            match msg.payload {
                IpcPayload::DisplayLine { .. } => {
                    display_lines.push(msg);
                }
                IpcPayload::Ack { .. } => {
                    ack_received = true;
                    break;
                }
                _ => {}
            }
        } else {
            break;
        }
    }

    assert!(
        !display_lines.is_empty(),
        "/config produced no DisplayLine messages"
    );
    assert!(ack_received, "No Ack received for /config");

    cleanup_socket(&path);
}

#[tokio::test]
async fn ipc_server_survives_client_disconnect() {
    let (path, _bus) = start_test_server().await;

    // Connect first client.
    {
        let mut client = TestClient::connect(&path).await.expect("connect failed");
        assert!(client.subscribe_and_wait_ready().await, "subscribe failed");
        // Drop client here (simulates disconnect).
    }

    // Wait for server to process disconnect.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Connect a NEW client. If server is still alive, this succeeds.
    let mut client2 = TestClient::connect(&path)
        .await
        .expect("second client connect failed — server crashed?");

    assert!(
        client2.subscribe_and_wait_ready().await,
        "second client subscribe failed"
    );

    cleanup_socket(&path);
}

#[tokio::test]
async fn ipc_multiple_clients_connect_simultaneously() {
    let (path, _bus) = start_test_server().await;

    // Connect 3 clients.
    let mut client1 = TestClient::connect(&path)
        .await
        .expect("client1 connect failed");
    let mut client2 = TestClient::connect(&path)
        .await
        .expect("client2 connect failed");
    let mut client3 = TestClient::connect(&path)
        .await
        .expect("client3 connect failed");

    // Each subscribes and gets SessionReady.
    assert!(
        client1.subscribe_and_wait_ready().await,
        "client1 subscribe failed"
    );
    assert!(
        client2.subscribe_and_wait_ready().await,
        "client2 subscribe failed"
    );
    assert!(
        client3.subscribe_and_wait_ready().await,
        "client3 subscribe failed"
    );

    // Each client sends Ping and gets Pong (no cross-contamination).
    let ping1_id = client1
        .send_msg(IpcPayload::Ping)
        .await
        .expect("client1 ping failed");
    let ping2_id = client2
        .send_msg(IpcPayload::Ping)
        .await
        .expect("client2 ping failed");
    let ping3_id = client3
        .send_msg(IpcPayload::Ping)
        .await
        .expect("client3 ping failed");

    let pong1 = client1.recv_msg().await.expect("client1 no pong");
    let pong2 = client2.recv_msg().await.expect("client2 no pong");
    let pong3 = client3.recv_msg().await.expect("client3 no pong");

    assert_eq!(pong1.id, ping1_id);
    assert!(matches!(pong1.payload, IpcPayload::Pong));

    assert_eq!(pong2.id, ping2_id);
    assert!(matches!(pong2.payload, IpcPayload::Pong));

    assert_eq!(pong3.id, ping3_id);
    assert!(matches!(pong3.payload, IpcPayload::Pong));

    cleanup_socket(&path);
}
