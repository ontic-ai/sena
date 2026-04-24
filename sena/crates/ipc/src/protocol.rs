use serde::{Deserialize, Serialize};
use serde_json::Value;

/// IPC request envelope sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    /// Unique request ID for matching responses.
    pub id: u64,
    /// Command name to dispatch.
    pub command: String,
    /// Command-specific payload (handler-defined structure).
    pub payload: Value,
}

/// IPC response envelope sent from server to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    /// Request ID this response corresponds to (0 for push events).
    pub id: u64,
    /// Whether the command succeeded.
    pub success: bool,
    /// Command-specific result payload or pushed event payload.
    pub payload: Value,
    /// Human-readable error when success = false.
    pub error: Option<String>,
}

impl IpcResponse {
    /// Create a success response.
    pub fn success(id: u64, payload: Value) -> Self {
        Self {
            id,
            success: true,
            payload,
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: u64, error: String) -> Self {
        Self {
            id,
            success: false,
            payload: Value::Null,
            error: Some(error),
        }
    }

    /// Create a push event (unsolicited response with id=0).
    pub fn push_event(payload: Value) -> Self {
        Self {
            id: 0,
            success: true,
            payload,
            error: None,
        }
    }
}
