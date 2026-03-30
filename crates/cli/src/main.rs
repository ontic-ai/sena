//! Sena CLI — application entry point.
//!
//! Default mode: interactive REPL (`cargo run`).
//! Legacy scripting mode: `cargo run -- query <type>` (no REPL, no stdin).
//! Standalone model picker: `cargo run -- models`.

mod display;
mod model_selector;
mod query;
mod shell;

use std::time::Duration;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("query") => query_mode(&args).await,
        Some("models") => model_selector::run().await,
        None => interactive_mode().await,
        _ => {
            print_usage();
            anyhow::bail!("Invalid arguments. Run with no args for interactive mode.")
        }
    }
}

/// Interactive mode: boot runtime, print banner, enter REPL.
/// Loops on restart signal so the user can hot-swap models without re-launching.
async fn interactive_mode() -> Result<()> {
    display::banner();

    loop {
        display::info("Booting runtime...");
        let runtime = runtime::boot().await?;
        display::success("Runtime ready.");

        let exit_reason = shell::run(runtime).await?;

        match exit_reason {
            shell::ShellExitReason::Quit => break,
            shell::ShellExitReason::Restart => {
                display::info("Restarting with new model...");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    Ok(())
}

/// Legacy scripting mode: single transparency query, print result, exit.
async fn query_mode(args: &[String]) -> Result<()> {
    if args.len() < 3 {
        eprintln!("Error: 'query' requires a type argument");
        eprintln!("Usage: cargo run -- query <observation|memory|explanation>");
        anyhow::bail!("Missing query type")
    }

    let query = query::parse_query_type(&args[2])?;
    let output = query::execute_query(query).await?;
    println!("{output}");
    Ok(())
}

fn print_usage() {
    eprintln!("Sena CLI");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  cargo run                     Start interactive mode (recommended)");
    eprintln!("  cargo run -- models           Pick an Ollama model (no REPL needed)");
    eprintln!("  cargo run -- query TYPE       Scripting: single query, print, exit");
}

#[allow(dead_code)]
async fn background_mode() -> Result<()> {
    let runtime = runtime::boot().await?;
    runtime::wait_for_sigint().await?;
    let shutdown_timeout = Duration::from_secs(runtime.config.shutdown_timeout_secs);
    runtime::shutdown(runtime, shutdown_timeout).await?;
    Ok(())
}
