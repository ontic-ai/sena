//! Sena CLI binary entrypoint.

use tracing::error;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt::init();

    // Boot the runtime
    let boot_result = runtime::boot().await.map_err(|e| {
        error!("Boot failed: {}", e);
        anyhow::anyhow!("Runtime boot failed: {}", e)
    })?;

    // Run supervision loop (blocks until shutdown)
    runtime::supervision_loop(boot_result).await.map_err(|e| {
        error!("Supervision loop failed: {}", e);
        anyhow::anyhow!("Runtime supervision failed: {}", e)
    })?;

    Ok(())
}
