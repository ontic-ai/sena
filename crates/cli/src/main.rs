//! Sena CLI — application entry point.
//!
//! The CLI has zero business logic. It delegates to runtime for boot/shutdown.
//! All actor wiring happens in runtime::boot().
//!
//! Supports two modes:
//! 1. Background mode: `sena` — boots runtime and waits for Ctrl+C
//! 2. Query mode: `sena query <type>` — sends transparency query, awaits response, exits

mod query;

use std::time::Duration;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 2 && args[1] == "query" {
        // Query mode: sena query <type>
        query_mode(&args).await
    } else if args.len() == 1 {
        // Background mode: sena
        background_mode().await
    } else {
        // Invalid arguments
        print_usage();
        anyhow::bail!("Invalid arguments. Use 'sena' or 'sena query <type>'")
    }
}

/// Background mode: boots runtime and waits for Ctrl+C.
async fn background_mode() -> Result<()> {
    // Boot runtime (actors are wired inside boot())
    let runtime = runtime::boot().await?;

    println!("Sena started. Press Ctrl+C to shutdown.");
    println!("To test Sena interactively, use: sena query <type>");

    // Wait for Ctrl+C
    runtime::wait_for_sigint().await?;

    println!("Shutdown signal received, stopping actors...");

    // Graceful shutdown with timeout from config
    let shutdown_timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    runtime::shutdown(runtime, shutdown_timeout).await?;

    println!("Sena stopped cleanly.");

    Ok(())
}

/// Query mode: sends a transparency query and displays the response.
///
/// Usage: sena query <type>
/// Valid types: observation, memory, explanation
async fn query_mode(args: &[String]) -> Result<()> {
    if args.len() < 3 {
        eprintln!("Error: query subcommand requires a query type");
        eprintln!("Usage: sena query <type>");
        eprintln!("Valid types: observation, memory, explanation");
        anyhow::bail!("Missing query type argument")
    }

    let query_type_arg = &args[2];
    let query = query::parse_query_type(query_type_arg)?;

    let output = query::execute_query(query).await?;
    println!("{}", output);

    Ok(())
}

/// Print usage information.
fn print_usage() {
    eprintln!("Sena CLI — transparency query interface");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  sena             Run in background mode (waits for Ctrl+C)");
    eprintln!("  sena query TYPE Query Sena's current state and exit");
    eprintln!();
    eprintln!("Query types:");
    eprintln!("  observation  What are you observing right now?");
    eprintln!("  memory       What do you remember about me?");
    eprintln!("  explanation  Why did you say that?");
}
