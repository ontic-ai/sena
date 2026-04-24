//! Event subscription IPC command handlers.

use async_trait::async_trait;
use ipc::{CommandHandler, IpcError};
use serde_json::Value;

/// Handler for "events.subscribe" command.
pub struct EventsSubscribeHandler;

#[async_trait]
impl CommandHandler for EventsSubscribeHandler {
    fn name(&self) -> &'static str {
        "events.subscribe"
    }

    fn description(&self) -> &'static str {
        "Subscribe to daemon event stream"
    }

    fn requires_boot(&self) -> bool {
        false
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        Ok(serde_json::json!({
            "subscribed": true,
            "stream": "events"
        }))
    }
}

/// Handler for "events.unsubscribe" command.
pub struct EventsUnsubscribeHandler;

#[async_trait]
impl CommandHandler for EventsUnsubscribeHandler {
    fn name(&self) -> &'static str {
        "events.unsubscribe"
    }

    fn description(&self) -> &'static str {
        "Unsubscribe from daemon event stream"
    }

    async fn handle(&self, _payload: Value) -> Result<Value, IpcError> {
        Ok(serde_json::json!({
            "subscribed": false,
            "stream": "events"
        }))
    }
}
