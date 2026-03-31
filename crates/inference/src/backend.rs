//! LLM backend abstraction trait.
//!
//! Defines the interface that any LLM backend (llama-cpp-rs, mock, etc.)
//! must implement. Allows swapping backends without changing actor logic.

use std::path::Path;

/// Compute backend type for model inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType {
    /// Apple Metal (macOS, Apple Silicon).
    Metal,
    /// NVIDIA CUDA (Windows/Linux).
    Cuda,
    /// CPU fallback (all platforms).
    Cpu,
}

impl BackendType {
    /// Auto-detect the best available backend for LLM inference.
    ///
    /// Detection logic:
    /// - macOS → Metal (Apple Silicon native acceleration)
    /// - Windows/Linux with NVIDIA GPU → CUDA
    /// - Otherwise → CPU fallback
    ///
    /// CUDA detection is lightweight and non-blocking: checks for presence of NVIDIA
    /// driver files without spawning external processes or loading heavy libraries.
    pub fn auto_detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            BackendType::Metal
        }

        #[cfg(target_os = "windows")]
        {
            if has_nvidia_gpu_windows() {
                BackendType::Cuda
            } else {
                BackendType::Cpu
            }
        }

        #[cfg(target_os = "linux")]
        {
            if has_nvidia_gpu_linux() {
                BackendType::Cuda
            } else {
                BackendType::Cpu
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            BackendType::Cpu
        }
    }
}

/// Check for NVIDIA GPU on Windows by detecting presence of CUDA driver DLL.
#[cfg(target_os = "windows")]
fn has_nvidia_gpu_windows() -> bool {
    // nvcuda.dll is the NVIDIA CUDA driver library installed with NVIDIA drivers.
    // On 64-bit Windows, it lives in System32.
    let system32 = std::env::var("SystemRoot")
        .map(|root| std::path::PathBuf::from(root).join("System32"))
        .unwrap_or_else(|_| std::path::PathBuf::from(r"C:\Windows\System32"));

    let nvcuda_dll = system32.join("nvcuda.dll");
    nvcuda_dll.exists()
}

/// Check for NVIDIA GPU on Linux by detecting NVIDIA driver proc file.
#[cfg(target_os = "linux")]
fn has_nvidia_gpu_linux() -> bool {
    // /proc/driver/nvidia/version exists when NVIDIA proprietary drivers are loaded.
    // This is a reliable and lightweight signal that CUDA-capable hardware is present.
    std::path::Path::new("/proc/driver/nvidia/version").exists()
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::Metal => write!(f, "Metal"),
            BackendType::Cuda => write!(f, "CUDA"),
            BackendType::Cpu => write!(f, "CPU"),
        }
    }
}

/// Parameters for inference generation.
#[derive(Debug, Clone)]
pub struct InferenceParams {
    /// Temperature for sampling (0.0 = deterministic, 1.0 = creative).
    pub temperature: f32,
    /// Top-p nucleus sampling threshold.
    pub top_p: f32,
    /// Maximum tokens to generate.
    pub max_tokens: usize,
}

impl Default for InferenceParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 2048,
        }
    }
}

/// Errors from LLM backend operations.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Model file not found or unreadable.
    #[error("model load failed: {0}")]
    ModelLoadFailed(String),

    /// Inference execution failed.
    #[error("inference failed: {0}")]
    InferenceFailed(String),

    /// Embedding generation failed.
    #[error("embedding failed: {0}")]
    EmbeddingFailed(String),

    /// Extraction failed.
    #[error("extraction failed: {0}")]
    ExtractionFailed(String),

    /// Backend not initialized (model not loaded yet).
    #[error("backend not initialized — model must be loaded first")]
    NotInitialized,
}

/// Trait for LLM backend implementations.
///
/// All methods are synchronous — callers must use `spawn_blocking`
/// to avoid blocking the async runtime.
pub trait LlmBackend: Send + 'static {
    /// Load model weights from the given GGUF path.
    fn load_model(
        &mut self,
        model_path: &Path,
        backend_type: BackendType,
    ) -> Result<(), BackendError>;

    /// Run text generation inference.
    fn infer(&self, prompt: &str, params: &InferenceParams) -> Result<String, BackendError>;

    /// Generate embedding vector for the given text.
    fn embed(&self, text: &str) -> Result<Vec<f32>, BackendError>;

    /// Extract structured facts from the given text.
    fn extract(&self, text: &str) -> Result<Vec<String>, BackendError>;

    /// Returns true if a model has been loaded.
    fn is_loaded(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_detect_returns_valid_backend() {
        // Should never panic and always return a valid backend type.
        let backend = BackendType::auto_detect();
        match backend {
            BackendType::Metal | BackendType::Cuda | BackendType::Cpu => {}
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_always_returns_metal() {
        assert_eq!(BackendType::auto_detect(), BackendType::Metal);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_returns_cuda_or_cpu() {
        let backend = BackendType::auto_detect();
        assert!(backend == BackendType::Cuda || backend == BackendType::Cpu);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_returns_cuda_or_cpu() {
        let backend = BackendType::auto_detect();
        assert!(backend == BackendType::Cuda || backend == BackendType::Cpu);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn nvidia_check_does_not_panic() {
        // Should never panic, even if driver files are missing
        let _ = has_nvidia_gpu_windows();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nvidia_check_does_not_panic() {
        // Should never panic, even if driver files are missing
        let _ = has_nvidia_gpu_linux();
    }

    #[test]
    fn backend_type_display() {
        assert_eq!(BackendType::Metal.to_string(), "Metal");
        assert_eq!(BackendType::Cuda.to_string(), "CUDA");
        assert_eq!(BackendType::Cpu.to_string(), "CPU");
    }

    #[test]
    fn inference_params_default() {
        let params = InferenceParams::default();
        assert!((params.temperature - 0.7).abs() < f32::EPSILON);
        assert!((params.top_p - 0.9).abs() < f32::EPSILON);
        assert_eq!(params.max_tokens, 2048);
    }

    #[test]
    fn backend_error_variants() {
        let e = BackendError::ModelLoadFailed("bad path".to_string());
        assert!(e.to_string().contains("bad path"));

        let e = BackendError::InferenceFailed("oom".to_string());
        assert!(e.to_string().contains("oom"));

        let e = BackendError::NotInitialized;
        assert!(e.to_string().contains("not initialized"));
    }

    // Compile-time check: BackendType is Send
    #[allow(dead_code)]
    fn assert_send<T: Send>() {}

    #[test]
    fn types_are_send() {
        assert_send::<BackendType>();
        assert_send::<InferenceParams>();
        assert_send::<BackendError>();
    }
}
