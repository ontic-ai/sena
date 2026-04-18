//! Sena CLI binary entrypoint.

mod daemon_ipc;
mod error;
mod shell;

use shell::Shell;
use tracing::error;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Route INFO-level (and above) logs to stdout by default.
    // RUST_LOG overrides the level when set.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .with_writer(std::io::stdout)
        .init();

    // Boot the runtime (steps 1-11 per architecture §4.1).
    let mut boot_result = runtime::boot().await.map_err(|e| {
        error!("Boot failed: {}", e);
        anyhow::anyhow!("Runtime boot failed: {}", e)
    })?;

    // Clone the bus before moving boot_result components.
    let bus = boot_result.bus.clone();

    // Extract IPC client handle for the shell.
    // The handle is wrapped in Option so we can take it cleanly.
    let ipc_handle = boot_result.ipc_client_handle.take().ok_or_else(|| {
        error!("IPC client handle missing after boot — critical failure");
        anyhow::anyhow!("IPC client handle unexpectedly absent after boot")
    })?;

    // Create IPC client for the shell.
    let ipc_client = daemon_ipc::IpcClient::new(ipc_handle);

    // Create shell with the IPC client.
    let shell = Shell::new(ipc_client);

    // Run supervision loop and shell concurrently — neither blocks the other.
    // When either completes (shutdown or shell exit) the other is cancelled.
    tokio::select! {
        result = runtime::supervision_loop(boot_result) => {
            if let Err(e) = result {
                error!("Supervision loop failed: {}", e);
                return Err(anyhow::anyhow!("Runtime supervision failed: {}", e));
            }
        }
        result = shell.run() => {
            // Shell exited — broadcast shutdown so supervision loop also exits.
            let _ = bus
                .broadcast(bus::Event::System(bus::SystemEvent::ShutdownSignal))
                .await;
            if let Err(e) = result {
                error!("Shell error: {}", e);
                return Err(anyhow::anyhow!("Shell failed: {}", e));
            }
        }
    }

    Ok(())
}
