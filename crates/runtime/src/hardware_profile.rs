//! Local hardware profiling for auto-configuration.
//!
//! Runs at boot, before actor launch. Determines available RAM, VRAM, and CPU
//! cores to compute a safe default inference token budget.
//!
//! All detection is local-only (no network). Results are written to config if
//! the user has not explicitly overridden the relevant setting.

use sysinfo::System;

/// Hardware profile collected at boot.
#[derive(Debug, Clone)]
pub struct HardwareProfile {
    pub total_ram_mb: u64,
    pub available_vram_mb: u64, // 0 if no dedicated GPU or detection failed
    pub cpu_cores: usize,
    #[allow(dead_code)]
    pub has_metal: bool, // macOS GPU
    #[allow(dead_code)]
    pub has_cuda: bool, // Windows/Linux NVIDIA GPU (best-effort)
}

impl Default for HardwareProfile {
    fn default() -> Self {
        Self {
            total_ram_mb: 4096,
            available_vram_mb: 0,
            cpu_cores: 2,
            has_metal: false,
            has_cuda: false,
        }
    }
}

/// Compute the recommended inference_max_tokens for this hardware.
///
/// Decision table:
/// - VRAM >= 8GB  → 8192
/// - VRAM >= 4GB  → 4096
/// - RAM  >= 16GB → 2048
/// - RAM  >= 8GB  → 1024
/// - else         → 512
pub fn recommended_tokens(profile: &HardwareProfile) -> u32 {
    if profile.available_vram_mb >= 8192 {
        8192
    } else if profile.available_vram_mb >= 4096 {
        4096
    } else if profile.total_ram_mb >= 16384 {
        2048
    } else if profile.total_ram_mb >= 8192 {
        1024
    } else {
        512
    }
}

/// Profile the current hardware synchronously.
///
/// Uses sysinfo for RAM/CPU. For VRAM, uses ioreg on macOS (via shell command),
/// nvidia-smi on Windows/Linux (if available). On failure, VRAM = 0.
///
/// This function runs BLOCKING — call it before the tokio runtime starts, or
/// wrap in spawn_blocking.
pub fn profile_hardware() -> HardwareProfile {
    let mut sys = System::new_all();
    sys.refresh_all();

    let total_ram_mb = sys.total_memory() / 1_048_576;
    let cpu_cores = sys.cpus().len();

    let (available_vram_mb, has_metal, has_cuda) = detect_vram();

    HardwareProfile {
        total_ram_mb,
        available_vram_mb,
        cpu_cores,
        has_metal,
        has_cuda,
    }
}

/// Detect VRAM and GPU type. Returns (vram_mb, has_metal, has_cuda).
/// Best-effort — never panics.
#[cfg(target_os = "macos")]
fn detect_vram() -> (u64, bool, bool) {
    use std::process::Command;

    // Try ioreg to get VRAM on macOS
    let output = Command::new("ioreg")
        .args(["-r", "-d1", "-w0", "-c", "IOPCIDevice"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Look for "VRAM,totalMB" = <value>
            for line in stdout.lines() {
                if line.contains("\"VRAM,totalMB\"") {
                    // Parse: "VRAM,totalMB" = 8192
                    if let Some(eq_pos) = line.find('=') {
                        let value_str = &line[eq_pos + 1..].trim();
                        if let Ok(vram_mb) = value_str.parse::<u64>() {
                            return (vram_mb, vram_mb > 0, false);
                        }
                    }
                }
            }
        }
    }

    (0, false, false)
}

#[cfg(target_os = "windows")]
fn detect_vram() -> (u64, bool, bool) {
    use std::process::Command;

    // Try nvidia-smi for NVIDIA GPU
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                if let Ok(vram_mb) = first_line.trim().parse::<u64>() {
                    return (vram_mb, false, vram_mb > 0);
                }
            }
        }
    }

    (0, false, false)
}

#[cfg(target_os = "linux")]
fn detect_vram() -> (u64, bool, bool) {
    use std::process::Command;

    // Try nvidia-smi for NVIDIA GPU
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                if let Ok(vram_mb) = first_line.trim().parse::<u64>() {
                    return (vram_mb, false, vram_mb > 0);
                }
            }
        }
    }

    (0, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_tokens_vram_8gb() {
        let profile = HardwareProfile {
            total_ram_mb: 16384,
            available_vram_mb: 8192,
            cpu_cores: 8,
            has_metal: false,
            has_cuda: true,
        };
        assert_eq!(recommended_tokens(&profile), 8192);
    }

    #[test]
    fn recommended_tokens_vram_4gb() {
        let profile = HardwareProfile {
            total_ram_mb: 16384,
            available_vram_mb: 4096,
            cpu_cores: 8,
            has_metal: false,
            has_cuda: true,
        };
        assert_eq!(recommended_tokens(&profile), 4096);
    }

    #[test]
    fn recommended_tokens_high_ram() {
        let profile = HardwareProfile {
            total_ram_mb: 16384,
            available_vram_mb: 0,
            cpu_cores: 8,
            has_metal: false,
            has_cuda: false,
        };
        assert_eq!(recommended_tokens(&profile), 2048);
    }

    #[test]
    fn recommended_tokens_medium_ram() {
        let profile = HardwareProfile {
            total_ram_mb: 8192,
            available_vram_mb: 0,
            cpu_cores: 4,
            has_metal: false,
            has_cuda: false,
        };
        assert_eq!(recommended_tokens(&profile), 1024);
    }

    #[test]
    fn recommended_tokens_low_ram() {
        let profile = HardwareProfile {
            total_ram_mb: 4096,
            available_vram_mb: 0,
            cpu_cores: 2,
            has_metal: false,
            has_cuda: false,
        };
        assert_eq!(recommended_tokens(&profile), 512);
    }

    #[test]
    fn profile_hardware_runs_without_panic() {
        let profile = profile_hardware();
        assert!(profile.total_ram_mb > 0, "RAM detection should always work");
        assert!(profile.cpu_cores > 0, "CPU detection should always work");
    }
}
