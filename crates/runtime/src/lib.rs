//! Boot sequence, actor registry, shutdown orchestration.

pub mod boot;
pub mod config;
pub mod registry;
pub mod shutdown;

pub use boot::{boot, BootError, Runtime};
pub use config::{save_config, ConfigError, SenaConfig};
pub use registry::ActorRegistry;
pub use shutdown::{shutdown, wait_for_sigint, ShutdownError};
