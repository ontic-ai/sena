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
    let boot_result = runtime::boot().await.map_err(|e| {
        error!("Boot failed: {}", e);
        anyhow::anyhow!("Runtime boot failed: {}", e)
    })?;

    // Clone the bus before moving boot_result into the supervision loop.
    let bus = boot_result.bus.clone();

    // Run supervision loop and shell concurrently — neither blocks the other.
    // When either completes (shutdown or shell exit) the other is cancelled.
    tokio::select! {
        result = runtime::supervision_loop(boot_result) => {
            if let Err(e) = result {
                error!("Supervision loop failed: {}", e);
                return Err(anyhow::anyhow!("Runtime supervision failed: {}", e));
            }
        }
        result = async {
            let shell = Shell::new().await?;
            shell.run().await
        } => {
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
