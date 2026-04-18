use crate::IpcError;
use async_trait::async_trait;
use serde_json::Value;

/// Trait for implementing IPC command handlers.
///
/// Each command handler defines a unique name, description, and async execution logic.
/// Handlers are registered with `CommandRegistry` and invoked when matching requests arrive.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Unique command name (e.g., "inference", "list_models").
    fn name(&self) -> &'static str;

    /// Human-readable description of what this command does.
    fn description(&self) -> &'static str;

    /// Whether this command requires the daemon to be fully booted.
    ///
    /// Defaults to `true`. Commands like "ping" or "status" may override to `false`.
    fn requires_boot(&self) -> bool {
        true
    }

    /// Execute the command with the provided payload.
    ///
    /// # Arguments
    ///
    /// * `payload` - JSON value from `IpcRequest.payload` (handler-specific structure)
    ///
    /// # Returns
    ///
    /// JSON value result on success, or `IpcError` on failure.
    async fn handle(&self, payload: Value) -> Result<Value, IpcError>;
}
