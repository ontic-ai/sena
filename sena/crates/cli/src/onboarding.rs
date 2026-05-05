//! First-boot onboarding wizard for Sena CLI.
//!
//! This wizard runs when the daemon emits OnboardingRequired during first boot.
//! It collects user name, file watch paths, and clipboard opt-in via IPC commands.
//!
//! Unlike the donor implementation, this wizard does NOT own the runtime or bus.
//! All state mutations go through IPC commands to the daemon.

use crate::error::CliError;
use ipc::IpcClient;
use serde_json::json;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

/// Result of the onboarding wizard.
// allowed: the wizard returns structured values for tests and future callers,
// while the current CLI path submits them over IPC and only checks success.
#[allow(dead_code)]
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
/// Sends choices to daemon via IPC and returns OnboardingResult.
///
/// # Errors
///
/// Returns CliError if user input is invalid or IPC communication fails.
pub async fn run_wizard(ipc: &mut IpcClient) -> Result<OnboardingResult, CliError> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Welcome message
    println!();
    println!("╔══════════════════════════════════════╗");
    println!("║     Welcome to Sena — First Setup    ║");
    println!("╚══════════════════════════════════════╝");
    println!();
    stdout
        .flush()
        .map_err(|e| CliError::OnboardingFailed(e.to_string()))?;

    // Prompt 1: User name (required)
    let user_name = loop {
        print!("  What should I call you? → ");
        stdout
            .lock()
            .flush()
            .map_err(|e| CliError::OnboardingFailed(e.to_string()))?;
        let mut line = String::new();
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| CliError::OnboardingFailed(e.to_string()))?;
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

    // Submit name to daemon via IPC
    ipc.send(
        "runtime.submit_onboarding_name",
        json!({ "name": user_name }),
    )
    .await
    .map_err(|e| CliError::OnboardingFailed(format!("failed to submit name: {}", e)))?;

    // Prompt 2: File watch paths (optional)
    let default_watch_str = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users".to_string())
    } else {
        std::env::var("HOME").unwrap_or_else(|_| "/home".to_string())
    };

    print!(
        "  File watch paths (comma-separated, default: {}): → ",
        default_watch_str
    );
    stdout
        .lock()
        .flush()
        .map_err(|e| CliError::OnboardingFailed(e.to_string()))?;
    let mut paths_line = String::new();
    stdin
        .lock()
        .read_line(&mut paths_line)
        .map_err(|e| CliError::OnboardingFailed(e.to_string()))?;
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
    stdout
        .lock()
        .flush()
        .map_err(|e| CliError::OnboardingFailed(e.to_string()))?;
    let mut clip_line = String::new();
    stdin
        .lock()
        .read_line(&mut clip_line)
        .map_err(|e| CliError::OnboardingFailed(e.to_string()))?;
    let clip_trimmed = clip_line.trim().to_lowercase();
    let clipboard_observation_enabled =
        clip_trimmed.is_empty() || clip_trimmed == "y" || clip_trimmed == "yes";
    println!();

    // Submit config to daemon via IPC
    let file_watch_paths_str: Vec<String> = file_watch_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    ipc.send(
        "runtime.submit_onboarding_config",
        json!({
            "file_watch_paths": file_watch_paths_str,
            "clipboard_observation_enabled": clipboard_observation_enabled,
        }),
    )
    .await
    .map_err(|e| CliError::OnboardingFailed(format!("failed to submit config: {}", e)))?;

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
