use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use bus::{BootProgressEvent, BootStatus};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

const QWEN_MANIFEST_JSON: &str = include_str!("../../../assets/models/qwen2.5-7b-instruct.json");
const NOMIC_EMBED_MANIFEST_JSON: &str =
    include_str!("../../../assets/models/nomic-embed-text-v1.5.json");
const PARAKEET_ENCODER_MANIFEST_JSON: &str =
    include_str!("../../../assets/models/parakeet-nemotron-encoder.json");
const PARAKEET_DECODER_MANIFEST_JSON: &str =
    include_str!("../../../assets/models/parakeet-nemotron-decoder.json");
const PARAKEET_TOKENIZER_MANIFEST_JSON: &str =
    include_str!("../../../assets/models/parakeet-nemotron-tokenizer.json");
const PIPER_VOICE_MANIFEST_JSON: &str =
    include_str!("../../../assets/models/piper-en-us-lessac-medium.json");
const PIPER_CONFIG_MANIFEST_JSON: &str =
    include_str!("../../../assets/models/piper-en-us-lessac-medium-config.json");
const OPENWAKEWORD_MANIFEST_JSON: &str =
    include_str!("../../../assets/models/openwakeword-hey-sena.json");
pub const PLACEHOLDER_SHA256: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelArtifact {
    pub filename: String,
    pub download_url: String,
    pub sha256: String,
}

impl ModelArtifact {
    pub fn cache_path(&self, cache_dir: &Path) -> PathBuf {
        cache_dir.join(&self.filename)
    }

    pub fn has_placeholder_checksum(&self) -> bool {
        self.sha256 == PLACEHOLDER_SHA256 || self.sha256.chars().all(|ch| ch == '0')
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedModel {
    pub id: String,
    pub family: String,
    pub filename: String,
    pub cache_subdir: String,
    pub parts: Vec<ModelArtifact>,
}

impl ManagedModel {
    pub fn cache_dir(&self, cache_root: &Path) -> PathBuf {
        cache_root.join("models").join(&self.cache_subdir)
    }

    pub fn artifact_paths(&self, cache_root: &Path) -> Vec<PathBuf> {
        let cache_dir = self.cache_dir(cache_root);
        self.parts
            .iter()
            .map(|artifact| artifact.cache_path(&cache_dir))
            .collect()
    }

    pub fn has_placeholder_checksum(&self) -> bool {
        self.parts.iter().any(ModelArtifact::has_placeholder_checksum)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationPlan {
    pub model: ManagedModel,
    pub cache_dir: PathBuf,
    pub artifact_paths: Vec<PathBuf>,
    pub verify_on_boot: bool,
    pub skip_checksum_if_placeholder: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootModelPlans {
    pub conversation: VerificationPlan,
    pub embedding: VerificationPlan,
    pub speech: Vec<VerificationPlan>,
}

impl BootModelPlans {
    pub fn speech_cache_dir(&self) -> Option<&Path> {
        self.speech.first().map(|plan| plan.cache_dir.as_path())
    }

    pub fn total_artifact_count(&self) -> usize {
        self.conversation.model.parts.len()
            + self.embedding.model.parts.len()
            + self
                .speech
                .iter()
                .map(|plan| plan.model.parts.len())
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactDisposition {
    Cached,
    Downloaded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactInventory {
    pub filename: String,
    pub path: PathBuf,
    pub disposition: ArtifactDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInventory {
    pub plan: VerificationPlan,
    pub artifacts: Vec<ArtifactInventory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootInventory {
    pub cache_root: PathBuf,
    pub conversation: ModelInventory,
    pub embedding: ModelInventory,
    pub speech: Vec<ModelInventory>,
}

#[derive(Debug, Error)]
pub enum ModelBootError {
    #[error("failed to resolve Sena data directory: {0}")]
    DirectoryResolution(String),
    #[error("failed to build download client: {0}")]
    Client(String),
    #[error("failed to create cache directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("failed to open {path}: {source}")]
    OpenFile { path: PathBuf, source: io::Error },
    #[error("failed to read {path}: {source}")]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("failed to write {path}: {source}")]
    WriteFile { path: PathBuf, source: io::Error },
    #[error("failed to remove {path}: {source}")]
    RemoveFile { path: PathBuf, source: io::Error },
    #[error("failed to rename {from} to {to}: {source}")]
    RenameFile {
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },
    #[error("request failed for {url}: {reason}")]
    Request { url: String, reason: String },
    #[error("download failed for {url} with HTTP status {status}")]
    HttpStatus { url: String, status: u16 },
    #[error("checksum mismatch for {path}: expected {expected}, found {actual}")]
    ChecksumMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
}

fn parse_manifest(manifest_json: &str) -> ManagedModel {
    serde_json::from_str(manifest_json).expect("embedded model manifest must deserialize")
}

fn build_verification_plan(cache_root: &Path, model: ManagedModel) -> VerificationPlan {
    VerificationPlan {
        cache_dir: model.cache_dir(cache_root),
        artifact_paths: model.artifact_paths(cache_root),
        verify_on_boot: true,
        skip_checksum_if_placeholder: model.has_placeholder_checksum(),
        model,
    }
}

pub fn qwen_manifest() -> ManagedModel {
    parse_manifest(QWEN_MANIFEST_JSON)
}

pub fn embed_manifest() -> ManagedModel {
    parse_manifest(NOMIC_EMBED_MANIFEST_JSON)
}

pub fn speech_manifests() -> Vec<ManagedModel> {
    [
        PARAKEET_ENCODER_MANIFEST_JSON,
        PARAKEET_DECODER_MANIFEST_JSON,
        PARAKEET_TOKENIZER_MANIFEST_JSON,
        PIPER_VOICE_MANIFEST_JSON,
        PIPER_CONFIG_MANIFEST_JSON,
        OPENWAKEWORD_MANIFEST_JSON,
    ]
    .into_iter()
    .map(parse_manifest)
    .collect()
}

pub fn verify_or_download_plan(cache_root: &Path) -> VerificationPlan {
    build_verification_plan(cache_root, qwen_manifest())
}

pub fn embed_verification_plan(cache_root: &Path) -> VerificationPlan {
    build_verification_plan(cache_root, embed_manifest())
}

pub fn speech_verification_plans(cache_root: &Path) -> Vec<VerificationPlan> {
    speech_manifests()
        .into_iter()
        .map(|model| build_verification_plan(cache_root, model))
        .collect()
}

pub fn boot_verification_plans(cache_root: &Path) -> BootModelPlans {
    BootModelPlans {
        conversation: verify_or_download_plan(cache_root),
        embedding: embed_verification_plan(cache_root),
        speech: speech_verification_plans(cache_root),
    }
}

pub fn resolve_sena_dir() -> Result<PathBuf, ModelBootError> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("sena"))
            .map_err(|_| ModelBootError::DirectoryResolution("APPDATA not set".to_owned()));
    }

    #[cfg(target_os = "macos")]
    {
        return std::env::var("HOME")
            .map(|home| {
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("sena")
            })
            .map_err(|_| ModelBootError::DirectoryResolution("HOME not set".to_owned()));
    }

    #[cfg(target_os = "linux")]
    {
        return std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".config").join("sena"))
            .map_err(|_| ModelBootError::DirectoryResolution("HOME not set".to_owned()));
    }

    #[allow(unreachable_code)]
    Err(ModelBootError::DirectoryResolution(
        "unsupported target for Sena data directory resolution".to_owned(),
    ))
}

pub async fn ensure_boot_inventory<F>(
    cache_root: &Path,
    mut reporter: F,
) -> Result<BootInventory, ModelBootError>
where
    F: FnMut(BootProgressEvent),
{
    let plans = boot_verification_plans(cache_root);
    let total_artifacts = plans.total_artifact_count().max(1);
    let client = Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|error| ModelBootError::Client(error.to_string()))?;

    reporter(models_event(BootStatus::Running, 0, Some("resolve cache root"), Some(cache_root)));

    let mut completed_artifacts = 0usize;
    let conversation = ensure_plan_inventory(
        &client,
        "conversation",
        plans.conversation.clone(),
        total_artifacts,
        &mut completed_artifacts,
        &mut reporter,
    )
    .await?;
    let embedding = ensure_plan_inventory(
        &client,
        "embedding",
        plans.embedding.clone(),
        total_artifacts,
        &mut completed_artifacts,
        &mut reporter,
    )
    .await?;

    let mut speech = Vec::with_capacity(plans.speech.len());
    for plan in plans.speech.iter().cloned() {
        speech.push(
            ensure_plan_inventory(
                &client,
                "speech",
                plan,
                total_artifacts,
                &mut completed_artifacts,
                &mut reporter,
            )
            .await?,
        );
    }

    reporter(models_event(
        BootStatus::Complete,
        100,
        None::<String>,
        None::<&Path>,
    ));

    Ok(BootInventory {
        cache_root: cache_root.to_path_buf(),
        conversation,
        embedding,
        speech,
    })
}

async fn ensure_plan_inventory<F>(
    client: &Client,
    category: &str,
    plan: VerificationPlan,
    total_artifacts: usize,
    completed_artifacts: &mut usize,
    reporter: &mut F,
) -> Result<ModelInventory, ModelBootError>
where
    F: FnMut(BootProgressEvent),
{
    fs::create_dir_all(&plan.cache_dir)
        .await
        .map_err(|source| ModelBootError::CreateDir {
            path: plan.cache_dir.clone(),
            source,
        })?;

    let part_count = plan.model.parts.len().max(1);
    let mut artifacts = Vec::with_capacity(plan.model.parts.len());

    for (index, artifact) in plan.model.parts.iter().enumerate() {
        let path = artifact.cache_path(&plan.cache_dir);
        let label = artifact_label(category, &plan.model, index + 1, part_count);
        reporter(models_event(
            BootStatus::Running,
            progress_percent(*completed_artifacts, total_artifacts),
            Some(label.as_str()),
            Some(path.as_path()),
        ));

        let disposition = ensure_artifact(client, artifact, &path).await?;
        *completed_artifacts += 1;

        reporter(models_event(
            BootStatus::Running,
            progress_percent(*completed_artifacts, total_artifacts),
            Some(label.as_str()),
            Some(path.as_path()),
        ));

        artifacts.push(ArtifactInventory {
            filename: artifact.filename.clone(),
            path,
            disposition,
        });
    }

    Ok(ModelInventory { plan, artifacts })
}

async fn ensure_artifact(
    client: &Client,
    artifact: &ModelArtifact,
    path: &Path,
) -> Result<ArtifactDisposition, ModelBootError> {
    if fs::try_exists(path)
        .await
        .map_err(|source| ModelBootError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?
    {
        if artifact.has_placeholder_checksum() || checksum_matches(path, &artifact.sha256).await? {
            return Ok(ArtifactDisposition::Cached);
        }

        fs::remove_file(path)
            .await
            .map_err(|source| ModelBootError::RemoveFile {
                path: path.to_path_buf(),
                source,
            })?;
        tracing::warn!(
            path = %path.display(),
            "invalid cached artifact removed before redownload"
        );
    }

    download_artifact(client, artifact, path).await?;
    Ok(ArtifactDisposition::Downloaded)
}

async fn download_artifact(
    client: &Client,
    artifact: &ModelArtifact,
    path: &Path,
) -> Result<(), ModelBootError> {
    let temp_path = path.with_extension("download");
    if fs::try_exists(&temp_path)
        .await
        .map_err(|source| ModelBootError::ReadFile {
            path: temp_path.clone(),
            source,
        })?
    {
        fs::remove_file(&temp_path)
            .await
            .map_err(|source| ModelBootError::RemoveFile {
                path: temp_path.clone(),
                source,
            })?;
    }

    let response = client
        .get(&artifact.download_url)
        .send()
        .await
        .map_err(|error| ModelBootError::Request {
            url: artifact.download_url.clone(),
            reason: error.to_string(),
        })?;

    if !response.status().is_success() {
        return Err(ModelBootError::HttpStatus {
            url: artifact.download_url.clone(),
            status: response.status().as_u16(),
        });
    }

    let mut response = response;
    let mut file = fs::File::create(&temp_path)
        .await
        .map_err(|source| ModelBootError::WriteFile {
            path: temp_path.clone(),
            source,
        })?;
    let mut hasher = Sha256::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ModelBootError::Request {
            url: artifact.download_url.clone(),
            reason: error.to_string(),
        })?
    {
        if !artifact.has_placeholder_checksum() {
            hasher.update(&chunk);
        }

        file.write_all(&chunk)
            .await
            .map_err(|source| ModelBootError::WriteFile {
                path: temp_path.clone(),
                source,
            })?;
    }

    file.flush()
        .await
        .map_err(|source| ModelBootError::WriteFile {
            path: temp_path.clone(),
            source,
        })?;
    drop(file);

    if !artifact.has_placeholder_checksum() {
        let actual = digest_hex(hasher.finalize().as_slice());
        if !actual.eq_ignore_ascii_case(&artifact.sha256) {
            let _ = fs::remove_file(&temp_path).await;
            return Err(ModelBootError::ChecksumMismatch {
                path: path.to_path_buf(),
                expected: artifact.sha256.clone(),
                actual,
            });
        }
    }

    fs::rename(&temp_path, path)
        .await
        .map_err(|source| ModelBootError::RenameFile {
            from: temp_path,
            to: path.to_path_buf(),
            source,
        })
}

async fn checksum_matches(path: &Path, expected: &str) -> Result<bool, ModelBootError> {
    let actual = sha256_hex(path).await?;
    Ok(actual.eq_ignore_ascii_case(expected))
}

async fn sha256_hex(path: &Path) -> Result<String, ModelBootError> {
    let mut file = fs::File::open(path)
        .await
        .map_err(|source| ModelBootError::OpenFile {
            path: path.to_path_buf(),
            source,
        })?;
    let mut buffer = [0u8; 16 * 1024];
    let mut hasher = Sha256::new();

    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|source| ModelBootError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(digest_hex(hasher.finalize().as_slice()))
}

fn digest_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn artifact_label(category: &str, model: &ManagedModel, part_index: usize, part_count: usize) -> String {
    if part_count > 1 {
        format!("{category}[{part_index}/{part_count}] {}", model.filename)
    } else {
        format!("{category} {}", model.filename)
    }
}

fn models_event(
    status: BootStatus,
    progress_percent: u8,
    subprocess: Option<impl Into<String>>,
    detail: Option<impl AsRef<Path>>,
) -> BootProgressEvent {
    BootProgressEvent {
        actor: "models".to_owned(),
        status,
        progress_percent,
        subprocess: subprocess.map(Into::into),
        detail: detail.map(|path| path.as_ref().display().to_string()),
    }
}

fn progress_percent(completed: usize, total: usize) -> u8 {
    ((completed.saturating_mul(100)) / total.max(1)) as u8
}

#[cfg(test)]
mod tests {
    use std::{path::Path, path::PathBuf};

    use super::{
        boot_verification_plans, qwen_manifest, resolve_sena_dir, speech_verification_plans,
        verify_or_download_plan,
    };
    use crate::config::HARDCODED_MODEL;

    #[test]
    fn manifest_matches_hardcoded_runtime_model() {
        let manifest = qwen_manifest();

        assert_eq!(manifest.id, HARDCODED_MODEL);
        assert_eq!(manifest.parts.len(), 2);
        assert!(manifest
            .parts
            .iter()
            .all(|artifact| artifact.filename.ends_with(".gguf")));
        assert_eq!(manifest.cache_subdir, "conversation");
    }

    #[test]
    fn real_checksums_require_hash_verification() {
        let plan = verify_or_download_plan(Path::new("cache-root"));

        assert!(plan.verify_on_boot);
        assert!(!plan.skip_checksum_if_placeholder);
        assert_eq!(plan.artifact_paths.len(), 2);
        assert!(plan.cache_dir.ends_with("conversation"));
        assert!(plan
            .artifact_paths
            .iter()
            .zip(plan.model.parts.iter())
            .all(|(path, artifact)| path.ends_with(&artifact.filename)));
    }

    #[test]
    fn boot_plans_include_embed_and_speech_models() {
        let plans = boot_verification_plans(Path::new("cache-root"));

        assert_eq!(plans.embedding.model.filename, "nomic-embed-text-v1.5.Q8_0.gguf");
        assert!(plans.embedding.cache_dir.ends_with("embed"));
        assert_eq!(plans.speech.len(), 6);
        assert!(plans.speech.iter().all(|plan| plan.cache_dir.ends_with("speech")));
    }

    #[test]
    fn placeholder_models_skip_checksum_verification() {
        let speech_plans = speech_verification_plans(Path::new("cache-root"));

        let piper_config = speech_plans
            .iter()
            .find(|plan| plan.model.filename == "en_US-lessac-medium.onnx.json")
            .expect("piper config plan must exist");
        let open_wakeword = speech_plans
            .iter()
            .find(|plan| plan.model.filename == "hey_sena.tflite")
            .expect("openwakeword plan must exist");
        let parakeet_encoder = speech_plans
            .iter()
            .find(|plan| plan.model.filename == "encoder.onnx")
            .expect("parakeet encoder plan must exist");

        assert!(piper_config.skip_checksum_if_placeholder);
        assert!(open_wakeword.skip_checksum_if_placeholder);
        assert!(!parakeet_encoder.skip_checksum_if_placeholder);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolves_sena_dir_from_appdata() {
        let previous = std::env::var_os("APPDATA");
        let temp_root = tempfile::tempdir().expect("create tempdir");
        unsafe {
            std::env::set_var("APPDATA", temp_root.path());
        }

        let resolved = resolve_sena_dir().expect("resolve sena dir");
        assert_eq!(resolved, PathBuf::from(temp_root.path()).join("sena"));

        match previous {
            Some(value) => unsafe {
                std::env::set_var("APPDATA", value);
            },
            None => unsafe {
                std::env::remove_var("APPDATA");
            },
        }
    }
}
