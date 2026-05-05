use crate::{CommandHandler, IpcError};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of command handlers with dispatch logic.
///
/// Handlers are registered by name. Duplicate registrations panic.
/// The registry includes a built-in "list_commands" meta-handler.
pub struct CommandRegistry {
    handlers: HashMap<&'static str, Arc<dyn CommandHandler>>,
}

impl CommandRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a command handler.
    ///
    /// # Panics
    ///
    /// Panics if a handler with the same name is already registered.
    pub fn register(&mut self, handler: Arc<dyn CommandHandler>) {
        let name = handler.name();
        if self.handlers.contains_key(name) {
            panic!(
                "duplicate command handler registration: '{}' already registered",
                name
            );
        }
        self.handlers.insert(name, handler);
    }

    /// Dispatch a command by name to its registered handler.
    ///
    /// # Errors
    ///
    /// Returns `IpcError::UnknownCommand` if no handler is registered for the command.
    /// Returns handler-specific errors propagated from `CommandHandler::handle`.
    pub async fn dispatch(&self, command: &str, payload: Value) -> Result<Value, IpcError> {
        if command == "list_commands" {
            let commands: Vec<Value> = self
                .list()
                .into_iter()
                .map(|(name, description, requires_boot)| {
                    json!({
                        "name": name,
                        "description": description,
                        "requires_boot": requires_boot,
                    })
                })
                .collect();

            return Ok(json!({ "commands": commands }));
        }

        let handler = self
            .handlers
            .get(command)
            .ok_or_else(|| IpcError::UnknownCommand(command.to_string()))?;

        handler.handle(payload).await
    }

    /// List all registered command handlers.
    ///
    /// Returns a vector of (name, description, requires_boot) tuples.
    pub fn list(&self) -> Vec<(&'static str, &'static str, bool)> {
        let mut items: Vec<(&'static str, &'static str, bool)> = self
            .handlers
            .values()
            .map(|h| (h.name(), h.description(), h.requires_boot()))
            .collect();

        // Built-in meta command.
        items.push(("list_commands", "List all available IPC commands", false));

        items.sort_by(|a, b| a.0.cmp(b.0));
        items
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct TestHandler;

    #[async_trait]
    impl CommandHandler for TestHandler {
        fn name(&self) -> &'static str {
            "test"
        }

        fn description(&self) -> &'static str {
            "Test handler"
        }

        async fn handle(&self, payload: Value) -> Result<Value, IpcError> {
            Ok(json!({ "echoed": payload }))
        }
    }

    #[tokio::test]
    async fn dispatch_routes_to_correct_handler() {
        let mut registry = CommandRegistry::new();
        registry.register(Arc::new(TestHandler));

        let result = registry
            .dispatch("test", json!({ "data": "hello" }))
            .await
            .unwrap();

        assert_eq!(result, json!({ "echoed": { "data": "hello" } }));
    }

    #[tokio::test]
    async fn unknown_command_returns_error() {
        let registry = CommandRegistry::new();
        let result = registry.dispatch("nonexistent", json!({})).await;

        assert!(matches!(result, Err(IpcError::UnknownCommand(_))));
    }

    #[test]
    #[should_panic(expected = "duplicate command handler registration")]
    fn duplicate_registration_panics() {
        let mut registry = CommandRegistry::new();
        registry.register(Arc::new(TestHandler));
        registry.register(Arc::new(TestHandler)); // Panic expected
    }

    #[tokio::test]
    async fn list_commands_returns_all_registered_handlers() {
        let mut registry = CommandRegistry::new();
        registry.register(Arc::new(TestHandler));

        let response = registry.dispatch("list_commands", json!({})).await.unwrap();
        let commands = response
            .get("commands")
            .and_then(|v| v.as_array())
            .expect("list_commands should return commands array");
        assert!(
            commands
                .iter()
                .any(|cmd| { cmd.get("name").and_then(|v| v.as_str()) == Some("test") })
        );
        assert!(
            commands
                .iter()
                .any(|cmd| { cmd.get("name").and_then(|v| v.as_str()) == Some("list_commands") })
        );
    }
}
