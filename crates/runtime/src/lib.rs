//! Boot sequence, actor registry, shutdown orchestration.

pub mod boot;
pub mod config;
pub mod models;
pub mod registry;
pub mod shutdown;
pub mod tray;

pub use boot::{boot, BootError, Runtime};
pub use config::{save_config, ConfigError, SenaConfig};
pub use models::{discover_models, ollama_models_dir, InferenceError, ModelRegistry};
pub use registry::ActorRegistry;
pub use shutdown::{shutdown, wait_for_sigint, ShutdownError};
pub use tray::TrayManager;
