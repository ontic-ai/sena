//! Sena CLI — application entry point.
//!
//! The CLI has zero business logic. It delegates to runtime for boot/shutdown.
//! All actor wiring happens in runtime::boot().

use std::time::Duration;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Boot runtime (actors are wired inside boot())
    let runtime = runtime::boot().await?;

    println!("Sena started. Press Ctrl+C to shutdown.");
    println!("To test Sena interactively, use: sena --interactive (implemented in Phase 4 M4.3)");

    // Wait for Ctrl+C
    runtime::wait_for_sigint().await?;

    println!("Shutdown signal received, stopping actors...");

    // Graceful shutdown with timeout from config
    let shutdown_timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    runtime::shutdown(runtime, shutdown_timeout).await?;

    println!("Sena stopped cleanly.");

    Ok(())
}
