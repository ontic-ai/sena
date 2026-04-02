//! Sena CLI — application entry point.

mod display;
mod model_selector;
mod onboarding;
mod query;
mod shell;
mod tui_state;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("query") => query::run_from_args(&args).await,
        Some("models") => model_selector::run().await,
        Some("cli") | Some("interactive") => shell::run_with_boot(true).await,
        None => shell::run_with_boot(false).await,
        _ => {
            eprintln!("Usage: sena [cli|query <type>|models]");
            anyhow::bail!("unknown argument")
        }
    }
}

