use std::time::Duration;

use bootstrap::{BootstrapApp, BootstrapLiveSession};
use bus::{BootProgressEvent, BootStatus};
use cli::CliView;
use runtime::{ArtifactDisposition, BootInventory, ModelInventory, RuntimeConfig, SenaRuntime};
use ui_core::{
    BootstrapSnapshot, CliSnapshot, ConversationLine, LoaderActorSnapshot, LoaderStatus,
    LoaderSubprocessSnapshot, SignalKind, SignalLine, SpeakerRole,
};

const RUNTIME_STAGE_PAUSE: Duration = Duration::from_millis(140);
const HANDOFF_PAUSE: Duration = Duration::from_millis(450);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").without_time().init();

    let runtime = SenaRuntime::new(RuntimeConfig::default());
    let attach_cli = runtime.config().attach_cli;
    let mut boot_ui = BootUiState::new(attach_cli);
    let loader = BootstrapApp::start_live(boot_ui.snapshot.clone());
    let mut loader_available = true;
    sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);

    boot_ui.apply(boot_event(
        "config",
        BootStatus::Running,
        40,
        Some("load runtime config"),
        Some(runtime.config().model.as_str()),
    ));
    sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);

    let cache_root = match runtime.resolve_data_dir() {
        Ok(path) => {
            boot_ui.apply(boot_event(
                "config",
                BootStatus::Complete,
                100,
                Some("resolve Sena data root"),
                Some(path.display().to_string()),
            ));
            sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
            path
        }
        Err(error) => {
            boot_ui.apply(boot_event(
                "config",
                BootStatus::Failed,
                100,
                Some("resolve Sena data root"),
                Some(error.to_string()),
            ));
            sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
            tokio::time::sleep(HANDOFF_PAUSE).await;
            finish_loader(loader, loader_available);
            eprintln!("failed to resolve Sena data directory: {error}");
            return;
        }
    };

    let boot_inventory = match runtime
        .ensure_boot_inventory(&cache_root, |event| {
            boot_ui.apply(event);
            sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
        })
        .await
    {
        Ok(inventory) => inventory,
        Err(error) => {
            boot_ui.apply(boot_event(
                "models",
                BootStatus::Failed,
                100,
                Some("managed model boot failed"),
                Some(error.to_string()),
            ));
            sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
            tokio::time::sleep(HANDOFF_PAUSE).await;
            finish_loader(loader, loader_available);
            eprintln!("managed model boot failed: {error}");
            return;
        }
    };

    boot_ui.apply(boot_event(
        "runtime",
        BootStatus::Running,
        30,
        Some("seed capability tree"),
        Some("SRI registry"),
    ));
    sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
    tokio::time::sleep(RUNTIME_STAGE_PAUSE).await;

    boot_ui.apply(boot_event(
        "runtime",
        BootStatus::Running,
        70,
        Some("warm conversation lane"),
        Some(runtime.config().model.as_str()),
    ));
    sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
    tokio::time::sleep(RUNTIME_STAGE_PAUSE).await;

    boot_ui.apply(boot_event(
        "runtime",
        BootStatus::Complete,
        100,
        Some("arm runtime lanes"),
        Some("runtime ready"),
    ));
    sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);

    if attach_cli {
        boot_ui.apply(boot_event(
            "cli",
            BootStatus::Running,
            70,
            Some("handoff to Sena CLI"),
            Some("developer subscriber surface"),
        ));
        sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
        tokio::time::sleep(HANDOFF_PAUSE).await;

        boot_ui.apply(boot_event(
            "cli",
            BootStatus::Complete,
            100,
            Some("attach Sena CLI"),
            Some("Ctrl+C stops the development subscriber surface"),
        ));
        sync_loader(&loader, &boot_ui.snapshot, &mut loader_available);
    } else {
        tokio::time::sleep(HANDOFF_PAUSE).await;
    }

    finish_loader(loader, loader_available);

    if attach_cli {
        let cli = CliView::start_live(build_cli_snapshot(runtime.config(), &boot_inventory));
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("failed to wait for Sena CLI shutdown: {error}");
        }
        if let Err(error) = cli.finish() {
            eprintln!("CLI surface unavailable: {error}");
        }
    }
}

fn build_cli_snapshot(config: &RuntimeConfig, boot_inventory: &BootInventory) -> CliSnapshot {
    let mut signals = vec![
        SignalLine {
            kind: SignalKind::Info,
            text: format!("attach_cli={}", config.attach_cli),
        },
        SignalLine {
            kind: SignalKind::Info,
            text: format!("sena-data root {}", boot_inventory.cache_root.display()),
        },
        SignalLine {
            kind: SignalKind::Sri,
            text: "seeded base capability shelves".to_owned(),
        },
    ];

    signals.extend(model_signal_lines("conversation", &boot_inventory.conversation));
    signals.extend(model_signal_lines("embedding", &boot_inventory.embedding));
    for model in &boot_inventory.speech {
        signals.extend(model_signal_lines("speech", model));
    }

    signals.push(SignalLine {
        kind: SignalKind::Info,
        text: "Sena CLI attached; press Ctrl+C to stop the development subscriber surface"
            .to_owned(),
    });

    CliSnapshot {
        conversation: vec![ConversationLine {
            role: SpeakerRole::Assistant,
            text: format!(
                "Bootstrapper handed off to Sena CLI with hardcoded model {}.",
                config.model
            ),
        }],
        signals,
    }
}

fn model_signal_lines(category: &str, model: &ModelInventory) -> Vec<SignalLine> {
    model
        .artifacts
        .iter()
        .map(|artifact| SignalLine {
            kind: SignalKind::Download,
            text: format!(
                "{} {} {}",
                category,
                disposition_label(artifact.disposition),
                artifact.path.display()
            ),
        })
        .collect()
}

fn disposition_label(disposition: ArtifactDisposition) -> &'static str {
    match disposition {
        ArtifactDisposition::Cached => "cached",
        ArtifactDisposition::Downloaded => "downloaded",
    }
}

fn boot_event(
    actor: &str,
    status: BootStatus,
    progress_percent: u8,
    subprocess: Option<&str>,
    detail: Option<impl Into<String>>,
) -> BootProgressEvent {
    BootProgressEvent {
        actor: actor.to_owned(),
        status,
        progress_percent,
        subprocess: subprocess.map(str::to_owned),
        detail: detail.map(Into::into),
    }
}

fn sync_loader(
    loader: &BootstrapLiveSession,
    snapshot: &BootstrapSnapshot,
    loader_available: &mut bool,
) {
    if !*loader_available {
        return;
    }

    if let Err(error) = loader.update(snapshot.clone()) {
        *loader_available = false;
        eprintln!("bootstrapper surface unavailable: {error}");
    }
}

fn finish_loader(loader: BootstrapLiveSession, loader_available: bool) {
    if !loader_available {
        return;
    }

    if let Err(error) = loader.finish() {
        eprintln!("bootstrapper surface unavailable: {error}");
    }
}

#[derive(Debug, Clone)]
struct BootUiState {
    snapshot: BootstrapSnapshot,
}

impl BootUiState {
    fn new(attach_cli: bool) -> Self {
        let mut actors = vec![boot_actor("config"), boot_actor("models"), boot_actor("runtime")];
        if attach_cli {
            actors.push(boot_actor("cli"));
        }

        Self {
            snapshot: BootstrapSnapshot {
                title: "Sena Bootstrapper".to_owned(),
                active_actor: Some("config".to_owned()),
                actors,
            },
        }
    }

    fn apply(&mut self, event: BootProgressEvent) {
        let Some(actor) = self
            .snapshot
            .actors
            .iter_mut()
            .find(|actor| actor.name == event.actor)
        else {
            return;
        };

        actor.status = loader_status(event.status);
        actor.progress_percent = event.progress_percent;

        if let Some(subprocess_name) = format_subprocess(&event) {
            if let Some(existing) = actor
                .subprocesses
                .iter_mut()
                .find(|subprocess| subprocess.name == subprocess_name)
            {
                existing.status = loader_status(event.status);
            } else {
                actor.subprocesses.push(LoaderSubprocessSnapshot {
                    name: subprocess_name,
                    status: loader_status(event.status),
                });
            }
        } else if matches!(event.status, BootStatus::Complete) {
            for subprocess in &mut actor.subprocesses {
                subprocess.status = LoaderStatus::Complete;
            }
        }

        self.snapshot.active_actor = active_actor_name(&self.snapshot);
    }
}

fn boot_actor(name: &str) -> LoaderActorSnapshot {
    LoaderActorSnapshot {
        name: name.to_owned(),
        status: LoaderStatus::Pending,
        progress_percent: 0,
        subprocesses: Vec::new(),
    }
}

fn loader_status(status: BootStatus) -> LoaderStatus {
    match status {
        BootStatus::Pending => LoaderStatus::Pending,
        BootStatus::Running => LoaderStatus::Running,
        BootStatus::Complete => LoaderStatus::Complete,
        BootStatus::Failed => LoaderStatus::Failed,
    }
}

fn format_subprocess(event: &BootProgressEvent) -> Option<String> {
    match (event.subprocess.as_deref(), event.detail.as_deref()) {
        (Some(subprocess), Some(detail)) => Some(format!("{} -> {}", subprocess, detail)),
        (Some(subprocess), None) => Some(subprocess.to_owned()),
        (None, Some(detail)) => Some(detail.to_owned()),
        (None, None) => None,
    }
}

fn active_actor_name(snapshot: &BootstrapSnapshot) -> Option<String> {
    snapshot
        .actors
        .iter()
        .find(|actor| actor.status == LoaderStatus::Failed)
        .or_else(|| {
            snapshot
                .actors
                .iter()
                .find(|actor| actor.status == LoaderStatus::Running)
        })
        .map(|actor| actor.name.clone())
}