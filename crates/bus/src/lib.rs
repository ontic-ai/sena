//! Event bus, actor trait, typed events.

pub mod actor;
pub mod bus;
pub mod events;

pub use actor::{Actor, ActorError};
pub use bus::{BusError, Event, EventBus};
pub use events::{
    CTPEvent, InferenceEvent, MemoryEvent, PlatformEvent, Priority, SoulEvent, SystemEvent,
};

#[cfg(test)]
mod integration_tests {
    use super::events::{CTPEvent, PlatformEvent, SystemEvent};
    use std::time::{Duration, Instant};
    use tokio::sync::{broadcast, mpsc};

    use crate::events::platform::{
        ClipboardDigest, FileEvent, FileEventKind, KeystrokeCadence, WindowContext,
    };

    use crate::events::ctp::ContextSnapshot;

    fn create_test_snapshot() -> ContextSnapshot {
        let now = Instant::now();
        ContextSnapshot {
            active_app: WindowContext {
                app_name: "TestApp".to_string(),
                window_title: None,
                bundle_id: None,
                timestamp: now,
            },
            recent_files: vec![],
            clipboard_digest: None,
            keystroke_cadence: KeystrokeCadence {
                events_per_minute: 0.0,
                burst_detected: false,
                idle_duration: Duration::from_secs(0),
            },
            session_duration: Duration::from_secs(0),
            inferred_task: None,
            timestamp: now,
        }
    }

    #[tokio::test]
    async fn system_events_through_broadcast_channel() {
        let (tx, mut rx1) = broadcast::channel::<SystemEvent>(16);
        let mut rx2 = tx.subscribe();

        tx.send(SystemEvent::ShutdownSignal).unwrap();

        let event1 = rx1.recv().await.unwrap();
        let event2 = rx2.recv().await.unwrap();

        assert!(matches!(event1, SystemEvent::ShutdownSignal));
        assert!(matches!(event2, SystemEvent::ShutdownSignal));
    }

    #[tokio::test]
    async fn platform_events_through_mpsc_channel() {
        let (tx, mut rx) = mpsc::channel::<PlatformEvent>(16);

        let event = PlatformEvent::WindowChanged(WindowContext {
            app_name: "Code".to_string(),
            window_title: Some("main.rs".to_string()),
            bundle_id: Some("com.microsoft.VSCode".to_string()),
            timestamp: Instant::now(),
        });

        tx.send(event).await.unwrap();
        let received = rx.recv().await.unwrap();

        if let PlatformEvent::WindowChanged(ctx) = received {
            assert_eq!(ctx.app_name, "Code");
        } else {
            panic!("Expected WindowChanged variant");
        }
    }

    #[tokio::test]
    async fn ctp_events_through_mpsc_channel() {
        let (tx, mut rx) = mpsc::channel::<CTPEvent>(16);

        let snapshot = create_test_snapshot();
        let event = CTPEvent::ThoughtEventTriggered(snapshot);

        tx.send(event).await.unwrap();
        let received = rx.recv().await.unwrap();

        if let CTPEvent::ThoughtEventTriggered(snap) = received {
            assert_eq!(snap.active_app.app_name, "TestApp");
        } else {
            panic!("Expected ThoughtEventTriggered variant");
        }
    }

    #[tokio::test]
    async fn ctp_events_through_broadcast_channel() {
        let (tx, mut rx1) = broadcast::channel::<CTPEvent>(16);
        let mut rx2 = tx.subscribe();

        let snapshot = create_test_snapshot();
        let event = CTPEvent::ContextSnapshotReady(snapshot);

        tx.send(event).unwrap();

        let event1 = rx1.recv().await.unwrap();
        let event2 = rx2.recv().await.unwrap();

        assert!(matches!(event1, CTPEvent::ContextSnapshotReady(_)));
        assert!(matches!(event2, CTPEvent::ContextSnapshotReady(_)));
    }

    #[tokio::test]
    async fn all_platform_event_variants_through_channel() {
        use std::path::PathBuf;
        let (tx, mut rx) = mpsc::channel::<PlatformEvent>(16);

        tx.send(PlatformEvent::WindowChanged(WindowContext {
            app_name: "App1".to_string(),
            window_title: None,
            bundle_id: None,
            timestamp: Instant::now(),
        }))
        .await
        .unwrap();

        tx.send(PlatformEvent::ClipboardChanged(ClipboardDigest {
            digest: Some("abc123".to_string()),
            char_count: 10,
            timestamp: Instant::now(),
        }))
        .await
        .unwrap();

        tx.send(PlatformEvent::FileEvent(FileEvent {
            path: PathBuf::from("/test/file.txt"),
            event_kind: FileEventKind::Modified,
            timestamp: Instant::now(),
        }))
        .await
        .unwrap();

        tx.send(PlatformEvent::KeystrokePattern(KeystrokeCadence {
            events_per_minute: 120.0,
            burst_detected: true,
            idle_duration: Duration::from_secs(1),
        }))
        .await
        .unwrap();

        assert!(matches!(
            rx.recv().await.unwrap(),
            PlatformEvent::WindowChanged(_)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            PlatformEvent::ClipboardChanged(_)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            PlatformEvent::FileEvent(_)
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            PlatformEvent::KeystrokePattern(_)
        ));
    }

    #[test]
    fn all_event_types_are_clone() {
        fn assert_clone<T: Clone>() {}

        assert_clone::<SystemEvent>();
        assert_clone::<PlatformEvent>();
        assert_clone::<CTPEvent>();
        assert_clone::<crate::MemoryEvent>();
        assert_clone::<crate::SoulEvent>();
    }

    #[test]
    fn all_event_types_are_send_and_static() {
        fn assert_send_static<T: Send + 'static>() {}

        assert_send_static::<SystemEvent>();
        assert_send_static::<PlatformEvent>();
        assert_send_static::<CTPEvent>();
        assert_send_static::<crate::MemoryEvent>();
        assert_send_static::<crate::SoulEvent>();
    }
}
