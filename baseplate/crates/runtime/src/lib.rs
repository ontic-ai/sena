mod app;
pub mod config;
pub mod model;
pub mod protocol;

pub use app::SenaRuntime;
pub use config::{RuntimeConfig, HARDCODED_MODEL};
pub use model::{
	ArtifactDisposition, ArtifactInventory, BootInventory, BootModelPlans, ManagedModel,
	ModelArtifact, ModelInventory, ModelBootError, VerificationPlan, boot_verification_plans,
	embed_manifest, embed_verification_plan, ensure_boot_inventory, qwen_manifest,
	resolve_sena_dir, speech_manifests, speech_verification_plans, verify_or_download_plan,
};
pub use protocol::{interpret_model_output, parse_output, OutputParseError, ProtocolMessage};
