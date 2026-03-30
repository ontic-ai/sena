//! Model selector for Sena CLI.
//!
//! Exposes two usage paths:
//!
//! 1. **Standalone** (`sena models`): `run()` — discovers models, prints menu,
//!    reads selection via its own sync stdin, persists to config.
//!
//! 2. **Shell** (`/models` inside REPL): `discover_and_print_menu()` prints the list
//!    and returns the model vec; the caller reads the user input via its own async
//!    stdin reader and passes it to `apply_selection()`.

use std::io;

use anyhow::{anyhow, Result};
use bus::events::inference::{ModelInfo, Quantization};
use inference::discover_models;
use platform::dirs::ollama_models_dir;

use crate::display;
use crate::display::{BOLD, CYAN, DIM, RESET};

// ── Shell-facing API ─────────────────────────────────────────────────────────

/// Discover models, print the numbered menu, and return the model list.
///
/// Called by `shell::run_models()` so the shell can read the selection via its
/// own async stdin reader rather than opening a second stdin handle.
///
/// # Errors
/// - Ollama directory not found
/// - No models discovered
pub async fn discover_and_print_menu(runtime: &runtime::boot::Runtime) -> Result<Vec<ModelInfo>> {
    let models_dir = ollama_models_dir()
        .map_err(|e| anyhow!("Could not find Ollama models directory: {}", e))?;

    display::info(&format!("Scanning: {}", models_dir.display()));

    let registry = discover_models(&models_dir).map_err(|e| {
        anyhow!(
            "Model discovery failed: {}. Run 'ollama pull <model>' first.",
            e
        )
    })?;

    let models = registry.models().to_vec();
    let current = &runtime.config.preferred_model;
    let default_name = registry.default_model().map(str::to_owned);

    print_menu(&models, current.as_deref(), default_name.as_deref());

    Ok(models)
}

/// Apply a selection string (number or name) and persist to config.
///
/// Returns the name of the selected model.
///
/// # Errors
/// - Selection out of range or not found
/// - Config save failure
pub async fn apply_selection(
    input: &str,
    models: &[ModelInfo],
    runtime: &runtime::boot::Runtime,
) -> Result<String> {
    let selected = resolve_selection(input, models)?;
    let name = selected.name.clone();

    let mut config = runtime.config.clone();
    config.preferred_model = Some(name.clone());
    runtime::save_config(&config).await?;

    Ok(name)
}

// ── Standalone API ────────────────────────────────────────────────────────────

/// Full interactive model selector for the `sena models` command.
///
/// Discovers models, prints menu, reads selection from its own stdin handle,
/// and persists the choice to config. Does NOT require the runtime to be booted.
pub async fn run() -> Result<()> {
    let models_dir = ollama_models_dir()
        .map_err(|e| anyhow!("Could not find Ollama models directory: {}", e))?;

    display::info(&format!("Scanning: {}", models_dir.display()));

    let registry = discover_models(&models_dir).map_err(|e| {
        anyhow!(
            "Model discovery failed: {}. Run 'ollama pull <model>' first.",
            e
        )
    })?;

    let models = registry.models();
    let current = runtime::config::load_or_create_config().await?;
    let default_name = registry.default_model().map(str::to_owned);

    print_menu(models, current.preferred_model.as_deref(), default_name.as_deref());

    display::prompt_inline("Enter number or model name (Enter to keep current): ");

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();

    if trimmed.is_empty() {
        display::info("No change made.");
        return Ok(());
    }

    let selected = resolve_selection(trimmed, models)?;
    let mut config = current;
    config.preferred_model = Some(selected.name.clone());
    runtime::save_config(&config).await?;

    println!();
    display::success(&format!("Selected: {}", selected.name));
    display::info("Saved to config. Restart Sena to use the new model.");
    println!();

    Ok(())
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Print the formatted model list table.
fn print_menu(models: &[ModelInfo], current: Option<&str>, default_name: Option<&str>) {
    println!();
    display::divider();
    for (i, model) in models.iter().enumerate() {
        let size = format_size(model.size_bytes);
        let quant = format_quantization(&model.quantization);
        let marker = if current.is_some_and(|c| c == model.name) {
            format!(" {CYAN}←{RESET}")
        } else {
            String::new()
        };
        println!(
            "  {BOLD}{CYAN}[{}]{RESET}  {:<30}  {:>7}  {DIM}{:<8}{RESET}{marker}",
            i + 1,
            model.name,
            size,
            quant
        );
    }
    display::divider();

    match current {
        Some(name) => display::info(&format!("Currently selected: {name}")),
        None => {
            let auto = default_name.unwrap_or("none");
            display::info(&format!("Currently selected: {auto} {DIM}(auto — largest){RESET}"));
        }
    }
    println!();
}

/// Resolve a user's raw input string to a model.
///
/// Accepts a 1-based index (e.g. `1`) or a model name (case-insensitive).
fn resolve_selection<'a>(input: &str, models: &'a [ModelInfo]) -> Result<&'a ModelInfo> {
    if let Ok(n) = input.parse::<usize>() {
        if n == 0 || n > models.len() {
            return Err(anyhow!(
                "selection {} is out of range — choose 1–{}",
                n,
                models.len()
            ));
        }
        return Ok(&models[n - 1]);
    }

    models
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(input))
        .ok_or_else(|| anyhow!("unknown model '{}' — enter a number or a model name", input))
}

/// Format byte size as a human-readable string.
fn format_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    }
}

/// Format a Quantization variant as a short string.
fn format_quantization(quant: &Quantization) -> &'static str {
    match quant {
        Quantization::Q4_0 => "Q4_0",
        Quantization::Q4_1 => "Q4_1",
        Quantization::Q5_0 => "Q5_0",
        Quantization::Q5_1 => "Q5_1",
        Quantization::Q8_0 => "Q8_0",
        Quantization::F16 => "F16",
        Quantization::F32 => "F32",
        Quantization::Unknown(_) => "unknown",
    }
}
