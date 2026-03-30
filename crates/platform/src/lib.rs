//! OS adapter trait + per-OS signal collection.

pub mod adapter;
pub mod dirs;
pub mod error;
pub mod factory;
pub mod platform_actor;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub use adapter::PlatformAdapter;
pub use dirs::{config_dir, detect_compute_backend, ollama_models_dir};
pub use error::PlatformError;
pub use factory::create_platform_adapter;
pub use platform_actor::PlatformActor;
