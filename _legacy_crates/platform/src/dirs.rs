//! OS-specific directory resolution.
//!
//! All OS-specific path logic lives here per architecture.md §5:
//! "PlatformAdapter is the only place OS-specific code lives."

use std::path::PathBuf;

use crate::error::PlatformError;

/// Returns the OS-specific config directory for Sena.
///
/// - macOS: `~/Library/Application Support/sena/`
/// - Windows: `%APPDATA%\sena\`
/// - Linux: `~/.config/sena/`
#[cfg(target_os = "macos")]
pub fn config_dir() -> Result<PathBuf, PlatformError> {
    let home = std::env::var("HOME")
        .map_err(|_| PlatformError::NotAvailable("HOME not set".to_string()))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("sena"))
}

/// Returns the OS-specific config directory for Sena.
///
/// - macOS: `~/Library/Application Support/sena/`
/// - Windows: `%APPDATA%\sena\`
/// - Linux: `~/.config/sena/`
#[cfg(target_os = "windows")]
pub fn config_dir() -> Result<PathBuf, PlatformError> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| PlatformError::NotAvailable("APPDATA not set".to_string()))?;
    Ok(PathBuf::from(appdata).join("sena"))
}

/// Returns the OS-specific config directory for Sena.
///
/// - macOS: `~/Library/Application Support/sena/`
/// - Windows: `%APPDATA%\sena\`
/// - Linux: `~/.config/sena/`
#[cfg(target_os = "linux")]
pub fn config_dir() -> Result<PathBuf, PlatformError> {
    let home = std::env::var("HOME")
        .map_err(|_| PlatformError::NotAvailable("HOME not set".to_string()))?;
    Ok(PathBuf::from(home).join(".config").join("sena"))
}

/// Returns the OS-specific Ollama models directory.
///
/// - macOS: `~/.ollama/models/`
/// - Windows: `%USERPROFILE%\.ollama\models\`
/// - Linux: `~/.ollama/models/`
#[cfg(target_os = "macos")]
pub fn ollama_models_dir() -> Result<PathBuf, PlatformError> {
    let home = std::env::var("HOME")
        .map_err(|_| PlatformError::NotAvailable("HOME not set".to_string()))?;
    Ok(PathBuf::from(home).join(".ollama").join("models"))
}

/// Returns the OS-specific Ollama models directory.
///
/// - macOS: `~/.ollama/models/`
/// - Windows: `%USERPROFILE%\.ollama\models\`
/// - Linux: `~/.ollama/models/`
#[cfg(target_os = "windows")]
pub fn ollama_models_dir() -> Result<PathBuf, PlatformError> {
    let userprofile = std::env::var("USERPROFILE")
        .map_err(|_| PlatformError::NotAvailable("USERPROFILE not set".to_string()))?;
    Ok(PathBuf::from(userprofile).join(".ollama").join("models"))
}

/// Returns the OS-specific Ollama models directory.
///
/// - macOS: `~/.ollama/models/`
/// - Windows: `%USERPROFILE%\.ollama\models\`
/// - Linux: `~/.ollama/models/`
#[cfg(target_os = "linux")]
pub fn ollama_models_dir() -> Result<PathBuf, PlatformError> {
    let home = std::env::var("HOME")
        .map_err(|_| PlatformError::NotAvailable("HOME not set".to_string()))?;
    Ok(PathBuf::from(home).join(".ollama").join("models"))
}

/// Detect the best available compute backend for the current platform.
///
/// Returns a string identifier:
/// - `"metal"` on macOS (Apple Silicon / Metal)
/// - `"cuda"` on Windows/Linux if NVIDIA GPU is available (nvidia-smi found)
/// - `"cpu"` as fallback on all platforms
#[cfg(target_os = "macos")]
pub fn detect_compute_backend() -> &'static str {
    "metal"
}

/// Detect the best available compute backend for the current platform.
#[cfg(target_os = "windows")]
pub fn detect_compute_backend() -> &'static str {
    if has_nvidia_gpu() {
        "cuda"
    } else {
        "cpu"
    }
}

/// Detect the best available compute backend for the current platform.
#[cfg(target_os = "linux")]
pub fn detect_compute_backend() -> &'static str {
    if has_nvidia_gpu() {
        "cuda"
    } else {
        "cpu"
    }
}

/// Check if nvidia-smi is available on PATH (indicates NVIDIA GPU + driver).
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn has_nvidia_gpu() -> bool {
    std::process::Command::new("nvidia-smi")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_returns_path() {
        let result = config_dir();
        assert!(result.is_ok());
        let path = result.expect("config_dir should return Ok");
        assert!(path.ends_with("sena"));
    }

    #[test]
    fn ollama_models_dir_returns_expected_path_structure() {
        let result = ollama_models_dir();
        // On CI or systems without the env var, this may fail — that's fine
        if let Ok(path) = result {
            let path_str = path.to_string_lossy();
            assert!(
                path_str.ends_with(".ollama\\models") || path_str.ends_with(".ollama/models"),
                "Path should end with .ollama/models, got: {}",
                path_str
            );
        }
    }

    #[test]
    fn detect_compute_backend_returns_valid_string() {
        let backend = detect_compute_backend();
        assert!(
            backend == "metal" || backend == "cuda" || backend == "cpu",
            "Expected metal, cuda, or cpu — got: {}",
            backend
        );
    }
}
