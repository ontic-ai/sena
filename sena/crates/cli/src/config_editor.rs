//! Config editor scaffold — IPC-driven configuration interface.
//!
//! This is a Phase 4 scaffold for editing daemon configuration via IPC.
//! Full implementation of interactive config editing is deferred to Phase 5+.

use crate::error::CliError;
use ipc::IpcClient;
use serde_json::json;
use std::io::{self, Write};
use tracing::info;

/// Config editor state.
pub struct ConfigEditor<'a> {
    ipc: &'a mut IpcClient,
}

impl<'a> ConfigEditor<'a> {
    /// Create a new config editor.
    pub fn new(ipc: &'a mut IpcClient) -> Self {
        Self { ipc }
    }

    /// Run the config editor.
    ///
    /// # Phase 4 Limitation
    ///
    /// This is a minimal scaffold that demonstrates IPC-driven config get/set.
    /// Full interactive config editing (list all keys, validate values, save to file)
    /// is planned for Phase 5+.
    pub async fn run(&mut self) -> Result<(), CliError> {
        info!("Config editor starting");

        println!("\n=== Sena Configuration Editor (Phase 4 Scaffold) ===\n");
        println!("This is a minimal config editor that uses IPC to get/set daemon config.");
        println!("Full interactive editing will be available in Phase 5+.\n");

        loop {
            println!("Options:");
            println!("  1. Get config value");
            println!("  2. Set config value");
            println!("  3. Exit config editor");
            print!("\nChoice (1-3): ");
            io::stdout().flush().map_err(CliError::Io)?;

            let mut input = String::new();
            io::stdin().read_line(&mut input).map_err(CliError::Io)?;

            match input.trim() {
                "1" => self.get_config_value().await?,
                "2" => self.set_config_value().await?,
                "3" => {
                    println!("Exiting config editor...");
                    break;
                }
                _ => {
                    println!("Invalid choice. Please enter 1, 2, or 3.\n");
                }
            }
        }

        info!("Config editor stopped");
        Ok(())
    }

    /// Get a config value via IPC.
    async fn get_config_value(&mut self) -> Result<(), CliError> {
        print!("Config key: ");
        io::stdout().flush().map_err(CliError::Io)?;

        let mut key = String::new();
        io::stdin().read_line(&mut key).map_err(CliError::Io)?;
        let key = key.trim();

        if key.is_empty() {
            println!("Key cannot be empty.\n");
            return Ok(());
        }

        match self.ipc.send("config.get", json!({"key": key})).await {
            Ok(response) => {
                println!("Value: {}\n", response);
            }
            Err(e) => {
                println!("Failed to get config: {}\n", e);
            }
        }

        Ok(())
    }

    /// Set a config value via IPC.
    async fn set_config_value(&mut self) -> Result<(), CliError> {
        print!("Config key: ");
        io::stdout().flush().map_err(CliError::Io)?;

        let mut key = String::new();
        io::stdin().read_line(&mut key).map_err(CliError::Io)?;
        let key = key.trim();

        if key.is_empty() {
            println!("Key cannot be empty.\n");
            return Ok(());
        }

        print!("Config value: ");
        io::stdout().flush().map_err(CliError::Io)?;

        let mut value = String::new();
        io::stdin().read_line(&mut value).map_err(CliError::Io)?;
        let value = value.trim();

        if value.is_empty() {
            println!("Value cannot be empty.\n");
            return Ok(());
        }

        match self
            .ipc
            .send("config.set", json!({"key": key, "value": value}))
            .await
        {
            Ok(_) => {
                println!("Config updated successfully.\n");
            }
            Err(e) => {
                println!("Failed to set config: {}\n", e);
            }
        }

        Ok(())
    }
}
