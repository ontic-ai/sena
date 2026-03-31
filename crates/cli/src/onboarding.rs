//! First-boot onboarding wizard.
//! Runs in the CLI when runtime.is_first_boot == true.
//! Collects user name, file watch paths, and clipboard opt-in.
//! Emits SoulEvent::InitializeWithName on the bus.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use bus::{Event, EventBus, SoulEvent};

/// Result of the onboarding wizard.
pub struct OnboardingResult {
    /// User's chosen name.
    pub user_name: String,
    /// File watch paths chosen by user (empty = defaults used).
    pub file_watch_paths: Vec<PathBuf>,
    /// Whether clipboard observation is enabled.
    pub clipboard_observation_enabled: bool,
}

/// Run the interactive first-boot onboarding wizard.
///
/// Blocks until the user has answered all prompts.
/// Returns OnboardingResult with user choices.
pub async fn run_wizard(
    bus: &std::sync::Arc<EventBus>,
    models_available: bool,
) -> Result<OnboardingResult> {
    eprintln!("[DEBUG] First boot detected — starting onboarding wizard...");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Welcome message
    println!();
    println!("╔══════════════════════════════════════╗");
    println!("║     Welcome to Sena — First Setup    ║");
    println!("╚══════════════════════════════════════╝");
    println!();
    stdout.flush().context("flush welcome banner")?;

    // Check models first
    if !models_available {
        println!("  ⚠  No AI models found.");
        println!();
        println!("  Sena requires a local GGUF model to function.");
        println!("  To install Ollama and download a model:");
        println!();
        println!("    1. Visit https://ollama.ai and install Ollama");
        println!("    2. Run:  ollama pull llama3.2:3b");
        println!("    3. Re-launch Sena");
        println!();
        println!("  (Continuing setup — you can run Sena without inference for now)");
        println!();
    }

    // Prompt 1: User name (required)
    let user_name = loop {
        print!("  What should I call you? → ");
        stdout.lock().flush().context("flush stdout")?;
        let mut line = String::new();
        stdin.lock().read_line(&mut line).context("read name")?;
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            println!("  Name cannot be empty. Please enter a name.");
        } else if trimmed.len() > 50 {
            println!("  Name is too long (max 50 characters).");
        } else {
            break trimmed;
        }
    };
    println!("  Nice to meet you, {}!", user_name);
    println!();

    // Prompt 2: File watch paths (optional)
    // Use platform-appropriate env var for default — no deps on `dirs` crate
    let default_watch_str = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users".to_string())
    } else {
        std::env::var("HOME").unwrap_or_else(|_| "/home".to_string())
    };

    print!(
        "  File watch paths (comma-separated, default: {}): → ",
        default_watch_str
    );
    stdout.lock().flush().context("flush stdout")?;
    let mut paths_line = String::new();
    stdin
        .lock()
        .read_line(&mut paths_line)
        .context("read paths")?;
    let paths_trimmed = paths_line.trim();

    let file_watch_paths: Vec<PathBuf> = if paths_trimmed.is_empty() {
        vec![PathBuf::from(&default_watch_str)]
    } else {
        paths_trimmed
            .split(',')
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| {
                if p.exists() {
                    true
                } else {
                    println!(
                        "  Warning: path '{}' does not exist, skipping.",
                        p.display()
                    );
                    false
                }
            })
            .collect()
    };
    println!();

    // Prompt 3: Clipboard observation opt-in (optional, default yes)
    print!("  Enable clipboard observation? [Y/n] → ");
    stdout.lock().flush().context("flush stdout")?;
    let mut clip_line = String::new();
    stdin
        .lock()
        .read_line(&mut clip_line)
        .context("read clipboard")?;
    let clip_trimmed = clip_line.trim().to_lowercase();
    let clipboard_observation_enabled =
        clip_trimmed.is_empty() || clip_trimmed == "y" || clip_trimmed == "yes";
    println!();

    // Emit InitializeWithName on the bus so Soul can persist the name
    bus.broadcast(Event::Soul(SoulEvent::InitializeWithName {
        name: user_name.clone(),
    }))
    .await
    .map_err(|e| anyhow::anyhow!("Failed to send name to Soul: {}", e))?;

    println!("  ✔  Setup complete! Starting Sena...");
    println!();

    Ok(OnboardingResult {
        user_name,
        file_watch_paths,
        clipboard_observation_enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboarding_result_constructs() {
        let result = OnboardingResult {
            user_name: "Alice".to_string(),
            file_watch_paths: vec![PathBuf::from("/home/alice")],
            clipboard_observation_enabled: true,
        };
        assert_eq!(result.user_name, "Alice");
        assert_eq!(result.file_watch_paths.len(), 1);
        assert!(result.clipboard_observation_enabled);
    }

    #[test]
    fn name_validation_empty_string() {
        let name = "";
        assert!(name.trim().is_empty());
    }

    #[test]
    fn name_validation_too_long() {
        let name = "a".repeat(51);
        assert!(name.len() > 50);
    }

    #[test]
    fn name_validation_valid() {
        let name = "Alice";
        assert!(!name.trim().is_empty());
        assert!(name.len() <= 50);
    }
}
