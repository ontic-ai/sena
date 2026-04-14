//! Context snapshot assembly logic.

use bus::ContextSnapshot;
use platform::{KeystrokeCadence, PlatformBackend, PlatformSignal, WindowContext};
use soul::SoulStore;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::CtpError;

/// Trait-based dependencies for snapshot assembly.
pub struct SnapshotAssembler {
    platform: Arc<dyn PlatformBackend>,
    #[allow(dead_code)] // TODO: integrate Soul summary into snapshot context
    soul: Arc<dyn SoulStore>,
    boot_time: Instant,
}

impl SnapshotAssembler {
    /// Create a new snapshot assembler with injected dependencies.
    pub fn new(platform: Arc<dyn PlatformBackend>, soul: Arc<dyn SoulStore>) -> Self {
        Self {
            platform,
            soul,
            boot_time: Instant::now(),
        }
    }

    /// Assemble a context snapshot from current platform state.
    ///
    /// In this stub implementation, we attempt to fetch all platform signals
    /// and assemble them into a snapshot. Missing signals result in sensible
    /// defaults (empty collections, None for optionals).
    pub async fn assemble(&self) -> Result<ContextSnapshot, CtpError> {
        // Fetch active window context (required).
        let active_app = match self.platform.active_window()? {
            PlatformSignal::Window(ctx) => ctx,
            _ => WindowContext {
                app_name: "unknown".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: Instant::now(),
            },
        };

        // Recent file events not yet supported by platform backend — default to empty.
        let recent_files = vec![];

        // Fetch clipboard digest (optional).
        let clipboard_digest = match self.platform.clipboard_content()? {
            PlatformSignal::Clipboard(digest) => Some(digest),
            _ => None,
        };

        // Fetch keystroke cadence (required).
        let keystroke_cadence = match self.platform.keystroke_cadence()? {
            PlatformSignal::Keystroke(cadence) => cadence,
            _ => KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
                timestamp: Instant::now(),
            },
        };

        // Calculate session duration since boot.
        let session_duration = self.boot_time.elapsed();

        // Assemble the snapshot.
        Ok(ContextSnapshot {
            active_app,
            recent_files,
            clipboard_digest,
            keystroke_cadence,
            session_duration,
            timestamp: Instant::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{ClipboardDigest, PlatformError, PlatformSignal};

    struct StubPlatform;

    impl PlatformBackend for StubPlatform {
        fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Window(WindowContext {
                app_name: "TestApp".to_string(),
                window_title: Some("Test Window".to_string()),
                bundle_id: None,
                timestamp: Instant::now(),
            }))
        }

        fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Clipboard(ClipboardDigest {
                digest: None,
                char_count: 0,
                timestamp: Instant::now(),
            }))
        }

        fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::Keystroke(KeystrokeCadence {
                events_per_minute: 60.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(10),
                timestamp: Instant::now(),
            }))
        }

        fn screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
            Ok(PlatformSignal::ScreenFrame(platform::ScreenFrame {
                width: 1,
                height: 1,
                rgb_data: vec![0, 0, 0],
                timestamp: Instant::now(),
            }))
        }
    }

    struct StubSoul;

    impl soul::SoulStore for StubSoul {
        fn write_event(
            &mut self,
            _description: String,
            _app_context: Option<String>,
            _timestamp: std::time::SystemTime,
        ) -> Result<u64, soul::SoulError> {
            Ok(1)
        }

        fn read_summary(
            &self,
            _max_events: usize,
            _max_chars: Option<usize>,
        ) -> Result<soul::SoulSummary, soul::SoulError> {
            Ok(soul::SoulSummary {
                content: "test summary".to_string(),
                event_count: 0,
            })
        }

        fn read_event(
            &self,
            _row_id: u64,
        ) -> Result<Option<soul::SoulEventRecord>, soul::SoulError> {
            Ok(None)
        }

        fn write_identity_signal(
            &mut self,
            _key: &str,
            _value: &str,
        ) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn read_identity_signal(&self, _key: &str) -> Result<Option<String>, soul::SoulError> {
            Ok(None)
        }

        fn read_all_identity_signals(&self) -> Result<Vec<soul::IdentitySignal>, soul::SoulError> {
            Ok(vec![])
        }

        fn increment_identity_counter(
            &mut self,
            _key: &str,
            _delta: u64,
        ) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn write_temporal_pattern(
            &mut self,
            _pattern: soul::TemporalPattern,
        ) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn read_temporal_patterns(&self) -> Result<Vec<soul::TemporalPattern>, soul::SoulError> {
            Ok(vec![])
        }

        fn initialize(&mut self) -> Result<(), soul::SoulError> {
            Ok(())
        }

        fn close(&mut self) -> Result<(), soul::SoulError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn assembler_creates_snapshot_with_stub_platform() {
        let platform = Arc::new(StubPlatform) as Arc<dyn PlatformBackend>;
        let soul = Arc::new(StubSoul) as Arc<dyn soul::SoulStore>;
        let assembler = SnapshotAssembler::new(platform, soul);

        let snapshot = assembler
            .assemble()
            .await
            .expect("snapshot assembly failed");

        assert_eq!(snapshot.active_app.app_name, "TestApp");
        assert_eq!(snapshot.keystroke_cadence.events_per_minute, 60.0);
        assert!(snapshot.clipboard_digest.is_some());
    }

    #[tokio::test]
    async fn assembler_handles_missing_active_window_with_default() {
        struct EmptyPlatform;

        impl PlatformBackend for EmptyPlatform {
            fn active_window(&self) -> Result<PlatformSignal, PlatformError> {
                Ok(PlatformSignal::Window(WindowContext {
                    app_name: "unknown".to_string(),
                    window_title: None,
                    bundle_id: None,
                    timestamp: Instant::now(),
                }))
            }

            fn clipboard_content(&self) -> Result<PlatformSignal, PlatformError> {
                Ok(PlatformSignal::Clipboard(ClipboardDigest {
                    digest: None,
                    char_count: 0,
                    timestamp: Instant::now(),
                }))
            }

            fn keystroke_cadence(&self) -> Result<PlatformSignal, PlatformError> {
                Ok(PlatformSignal::Keystroke(KeystrokeCadence {
                    events_per_minute: 0.0,
                    burst_detected: false,
                    idle_duration: Duration::from_secs(0),
                    timestamp: Instant::now(),
                }))
            }

            fn screen_frame(&self) -> Result<PlatformSignal, PlatformError> {
                Ok(PlatformSignal::ScreenFrame(platform::ScreenFrame {
                    width: 1,
                    height: 1,
                    rgb_data: vec![0, 0, 0],
                    timestamp: Instant::now(),
                }))
            }
        }

        let platform = Arc::new(EmptyPlatform) as Arc<dyn PlatformBackend>;
        let soul = Arc::new(StubSoul) as Arc<dyn soul::SoulStore>;
        let assembler = SnapshotAssembler::new(platform, soul);

        let snapshot = assembler
            .assemble()
            .await
            .expect("snapshot assembly failed");

        assert_eq!(snapshot.active_app.app_name, "unknown");
        assert_eq!(snapshot.keystroke_cadence.events_per_minute, 0.0);
    }
}
