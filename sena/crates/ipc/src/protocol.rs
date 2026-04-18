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
    /// Response status.
    #[serde(flatten)]
    pub status: ResponseStatus,
}

/// Response status: success or error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ResponseStatus {
    /// Command succeeded.
    Success {
        /// Command-specific result payload.
        result: Value,
    },
    /// Command failed.
    Error {
        /// Error message.
        error: String,
    },
}

impl IpcResponse {
    /// Create a success response.
    pub fn success(id: u64, result: Value) -> Self {
        Self {
            id,
            status: ResponseStatus::Success { result },
        }
    }

    /// Create an error response.
    pub fn error(id: u64, error: String) -> Self {
        Self {
            id,
            status: ResponseStatus::Error { error },
        }
    }

    /// Create a push event (unsolicited response with id=0).
    pub fn push_event(result: Value) -> Self {
        Self {
            id: 0,
            status: ResponseStatus::Success { result },
        }
    }
}
