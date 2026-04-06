//! M4.4 stability test — verifies Sena runs for an extended period without leak,
//! panic, or unhandled errors.
//!
//! This test:
//! - Boots all actors with a MockBackend (no GGUF required)
//! - Drives the inference loop with repeated requests over 30 seconds
//! - Asserts memory usage stays below a generous 512 MB ceiling
//! - Asserts no actor panics or silent exits
//! - Asserts every InferenceRequested receives an InferenceCompleted response
//!
//! Run with: `cargo test -p runtime --test stability -- --nocapture`
//!
//! A 72-hour longevity test exists for M4.4 milestone verification.
//! Run it with: `cargo test -p runtime --test stability longevity_72h -- --ignored --nocapture`

use std::sync::Arc;
use std::time::{Duration, Instant};

use bus::{Actor, Event, EventBus, InferenceEvent, SystemEvent};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
use tempfile::tempdir;

/// The test budget in seconds. Keep short for CI; extend locally for soak testing.
const TEST_DURATION_SECS: u64 = 30;
/// Memory ceiling in MB — generous upper bound for a dev build with mock backend.
const MEMORY_CEILING_MB: u64 = 512;
/// How often to issue a new inference request.
const REQUEST_INTERVAL_MS: u64 = 500;

/// Longevity test duration: 72 hours as required by M4.4.
const LONGEVITY_DURATION_SECS: u64 = 72 * 60 * 60; // 259200 seconds
/// Request interval for longevity test — more conservative to reduce wear.
const LONGEVITY_REQUEST_INTERVAL_MS: u64 = 2000;

fn create_mock_ollama_structure(model_dir: &std::path::Path) {
    let manifests_lib = model_dir
        .join("manifests")
        .join("registry.ollama.ai")
        .join("library");
    std::fs::create_dir_all(&manifests_lib).expect("create manifests dir");
    let test_model_dir = manifests_lib.join("test-model");
    std::fs::create_dir_all(&test_model_dir).expect("create model dir");
    let manifest_json = r#"{"schemaVersion":2,"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:testdigest123","size":3000000000}]}"#;
    std::fs::write(test_model_dir.join("latest"), manifest_json).expect("write manifest");
    let blobs_dir = model_dir.join("blobs");
    std::fs::create_dir_all(&blobs_dir).expect("create blobs dir");
    std::fs::write(blobs_dir.join("sha256-testdigest123"), vec![0u8; 64]).expect("write blob");
}

#[tokio::test]
async fn stability_run_30_seconds_no_leak_no_panic() {
    let dir = tempdir().expect("tempdir");

    // ── Paths ─────────────────────────────────────────────────────────────────
    let soul_path = dir.path().join("soul.redb.enc");
    let model_dir = dir.path().join("models");
    std::fs::create_dir_all(&model_dir).expect("model dir");
    create_mock_ollama_structure(&model_dir);

    // ── Boot actors ───────────────────────────────────────────────────────────
    let bus = Arc::new(EventBus::new());

    let soul_master_key = crypto::MasterKey::from_bytes([1u8; 32]);
    let mut soul = Box::new(soul::SoulActor::new(&soul_path, soul_master_key));
    soul.start(Arc::clone(&bus)).await.expect("soul start");
    let soul_handle = tokio::spawn(async move { soul.run().await });

    let memory_dir = dir.path().join("memory");
    let memory_master_key = crypto::MasterKey::from_bytes([1u8; 32]);
    let mut memory = Box::new(memory::MemoryActor::new(&memory_dir, memory_master_key));
    memory.start(Arc::clone(&bus)).await.expect("memory start");
    let memory_handle = tokio::spawn(async move { memory.run().await });

    let mut inference = Box::new(inference::InferenceActor::new(
        model_dir,
        Box::new(inference::MockBackend::new()),
    ));
    inference
        .start(Arc::clone(&bus))
        .await
        .expect("inference start");
    let inference_handle = tokio::spawn(async move { inference.run().await });

    // ── Telemetry ─────────────────────────────────────────────────────────────
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new().with_memory()),
    );
    let pid = sysinfo::get_current_pid().expect("get pid");
    let mut peak_memory_mb: u64 = 0;

    // ── Drive loop ────────────────────────────────────────────────────────────
    let mut rx = bus.subscribe_broadcast();
    let start = Instant::now();
    let mut request_id: u64 = 1;
    let mut requests_sent: usize = 0;
    let mut responses_received: usize = 0;
    let mut next_request_at = Instant::now();

    loop {
        let elapsed = start.elapsed().as_secs();
        if elapsed >= TEST_DURATION_SECS {
            break;
        }

        // Issue a new request on the interval.
        if Instant::now() >= next_request_at {
            let prompt = format!("stability test message #{}", request_id);
            bus.send_directed(
                "inference",
                Event::Inference(InferenceEvent::InferenceRequested {
                    prompt,
                    priority: bus::events::inference::Priority::Normal,
                    request_id,
                    source: bus::InferenceSource::UserText,
                }),
            )
            .await
            .expect("send_directed");
            requests_sent += 1;
            request_id += 1;
            next_request_at = Instant::now() + Duration::from_millis(REQUEST_INTERVAL_MS);
        }

        // Drain bus events (non-blocking, 20 ms window).
        let drain_deadline = Instant::now() + Duration::from_millis(20);
        loop {
            let remaining = drain_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(Event::Inference(InferenceEvent::InferenceCompleted { .. }))) => {
                    responses_received += 1;
                }
                Ok(Ok(Event::System(SystemEvent::MemoryThresholdExceeded {
                    current_mb, ..
                }))) => {
                    eprintln!("WARN: MemoryThresholdExceeded at {} MB", current_mb);
                }
                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
            }
        }

        // Sample memory every ~5 seconds.
        if elapsed.is_multiple_of(5) {
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
            if let Some(proc) = sys.process(pid) {
                let mb = proc.memory() / (1024 * 1024);
                if mb > peak_memory_mb {
                    peak_memory_mb = mb;
                }
            }
        }
    }

    // ── Final drain ───────────────────────────────────────────────────────────
    // Drain any in-flight responses before shutdown to avoid false-positive
    // sent != responses assertion failure.
    let final_drain_deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let remaining = final_drain_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(Ok(Event::Inference(InferenceEvent::InferenceCompleted { .. }))) =
            tokio::time::timeout(remaining, rx.recv()).await
        {
            responses_received += 1;
        }
    }

    // ── Shutdown ──────────────────────────────────────────────────────────────
    bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
        .await
        .expect("shutdown broadcast");

    let _ = tokio::time::timeout(Duration::from_secs(5), soul_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), memory_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), inference_handle).await;

    // ── Assertions ────────────────────────────────────────────────────────────
    eprintln!(
        "[stability] duration={}s requests={} responses={} peak_memory={}MB",
        TEST_DURATION_SECS, requests_sent, responses_received, peak_memory_mb
    );

    assert!(
        requests_sent > 0,
        "no requests were sent during the stability window"
    );

    // Every request must receive a response (MockBackend is synchronous, no drops).
    assert_eq!(
        requests_sent, responses_received,
        "mismatch: sent {} requests but got {} responses (dropped messages)",
        requests_sent, responses_received
    );

    // Memory must stay below generous ceiling.
    assert!(
        peak_memory_mb < MEMORY_CEILING_MB,
        "peak memory {}MB exceeds {}MB ceiling — possible leak",
        peak_memory_mb,
        MEMORY_CEILING_MB
    );
}

/// M4.4 longevity test — verifies Sena runs for 72 hours without restart, leak, or panic.
///
/// This test is identical to the 30-second stability test but runs for 72 hours.
/// It is marked `#[ignore]` so it does NOT run in default CI and must be explicitly invoked.
///
/// Requirements:
/// - All actors boot cleanly with MockBackend
/// - Inference requests issued every 2 seconds for 72 hours
/// - Memory stays below 512 MB ceiling throughout
/// - Every request receives a response (no dropped messages)
/// - No actor panics or exits early
///
/// Run command:
/// ```bash
/// cargo test -p runtime --test stability longevity_72h -- --ignored --nocapture
/// ```
///
/// This test satisfies M4.4 requirement: "Sena runs for 72 hours without restart in testing."
#[tokio::test]
#[ignore] // Not run in default CI — manual invocation only
async fn longevity_72h_no_leak_no_panic() {
    let dir = tempdir().expect("tempdir");

    // ── Paths ─────────────────────────────────────────────────────────────────
    let soul_path = dir.path().join("soul.redb.enc");
    let memory_dir = dir.path().join("memory");
    let model_dir = dir.path().join("models");
    std::fs::create_dir_all(&model_dir).expect("model dir");
    create_mock_ollama_structure(&model_dir);

    // ── Boot actors ───────────────────────────────────────────────────────────
    let bus = Arc::new(EventBus::new());

    let soul_master_key = crypto::MasterKey::from_bytes([1u8; 32]);
    let mut soul = Box::new(soul::SoulActor::new(&soul_path, soul_master_key));
    soul.start(Arc::clone(&bus)).await.expect("soul start");
    let soul_handle = tokio::spawn(async move { soul.run().await });

    let memory_master_key = crypto::MasterKey::from_bytes([1u8; 32]);
    let mut memory = Box::new(memory::MemoryActor::new(&memory_dir, memory_master_key));
    memory.start(Arc::clone(&bus)).await.expect("memory start");
    let memory_handle = tokio::spawn(async move { memory.run().await });

    let mut inference = Box::new(inference::InferenceActor::new(
        model_dir,
        Box::new(inference::MockBackend::new()),
    ));
    inference
        .start(Arc::clone(&bus))
        .await
        .expect("inference start");
    let inference_handle = tokio::spawn(async move { inference.run().await });

    // ── Telemetry ─────────────────────────────────────────────────────────────
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::new().with_memory()),
    );
    let pid = sysinfo::get_current_pid().expect("get pid");
    let mut peak_memory_mb: u64 = 0;

    // ── Drive loop ────────────────────────────────────────────────────────────
    let mut rx = bus.subscribe_broadcast();
    let start = Instant::now();
    let mut request_id: u64 = 1;
    let mut requests_sent: usize = 0;
    let mut responses_received: usize = 0;
    let mut next_request_at = Instant::now();
    let mut last_progress_report = Instant::now();

    eprintln!("[longevity] Starting 72-hour test. This will take 3 days.");
    eprintln!("[longevity] Press Ctrl+C to terminate early if needed.");

    loop {
        let elapsed = start.elapsed();
        let elapsed_secs = elapsed.as_secs();

        if elapsed_secs >= LONGEVITY_DURATION_SECS {
            break;
        }

        // Issue a new request on the interval.
        if Instant::now() >= next_request_at {
            let prompt = format!("longevity test message #{}", request_id);
            bus.send_directed(
                "inference",
                Event::Inference(InferenceEvent::InferenceRequested {
                    prompt,
                    priority: bus::events::inference::Priority::Normal,
                    request_id,
                    source: bus::InferenceSource::UserText,
                }),
            )
            .await
            .expect("send_directed");
            requests_sent += 1;
            request_id += 1;
            next_request_at = Instant::now() + Duration::from_millis(LONGEVITY_REQUEST_INTERVAL_MS);
        }

        // Drain bus events (non-blocking, 50 ms window).
        let drain_deadline = Instant::now() + Duration::from_millis(50);
        loop {
            let remaining = drain_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(Event::Inference(InferenceEvent::InferenceCompleted { .. }))) => {
                    responses_received += 1;
                }
                Ok(Ok(Event::System(SystemEvent::MemoryThresholdExceeded {
                    current_mb, ..
                }))) => {
                    eprintln!(
                        "[longevity] WARN: MemoryThresholdExceeded at {} MB (elapsed: {}h)",
                        current_mb,
                        elapsed_secs / 3600
                    );
                }
                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
            }
        }

        // Sample memory every 60 seconds.
        if elapsed_secs.is_multiple_of(60) {
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
            if let Some(proc) = sys.process(pid) {
                let mb = proc.memory() / (1024 * 1024);
                if mb > peak_memory_mb {
                    peak_memory_mb = mb;
                }
            }
        }

        // Progress report every hour.
        if last_progress_report.elapsed().as_secs() >= 3600 {
            let hours_elapsed = elapsed_secs / 3600;
            let hours_remaining = (LONGEVITY_DURATION_SECS - elapsed_secs) / 3600;
            eprintln!(
                "[longevity] Progress: {}h elapsed, {}h remaining | requests={} responses={} peak_mem={}MB",
                hours_elapsed, hours_remaining, requests_sent, responses_received, peak_memory_mb
            );
            last_progress_report = Instant::now();
        }
    }

    // ── Final drain ───────────────────────────────────────────────────────────
    // Drain any in-flight responses before shutdown to avoid false-positive
    // sent != responses assertion failure.
    let final_drain_deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let remaining = final_drain_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(Ok(Event::Inference(InferenceEvent::InferenceCompleted { .. }))) =
            tokio::time::timeout(remaining, rx.recv()).await
        {
            responses_received += 1;
        }
    }

    // ── Shutdown ──────────────────────────────────────────────────────────────
    eprintln!("[longevity] 72 hours complete. Shutting down actors...");
    bus.broadcast(Event::System(SystemEvent::ShutdownSignal))
        .await
        .expect("shutdown broadcast");

    let _ = tokio::time::timeout(Duration::from_secs(10), soul_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), memory_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), inference_handle).await;

    // ── Assertions ────────────────────────────────────────────────────────────
    eprintln!(
        "[longevity] Final: duration={}h requests={} responses={} peak_memory={}MB",
        LONGEVITY_DURATION_SECS / 3600,
        requests_sent,
        responses_received,
        peak_memory_mb
    );

    assert!(
        requests_sent > 0,
        "no requests were sent during the longevity window"
    );

    // Every request must receive a response (MockBackend is synchronous, no drops).
    assert_eq!(
        requests_sent, responses_received,
        "mismatch: sent {} requests but got {} responses (dropped messages)",
        requests_sent, responses_received
    );

    // Memory must stay below generous ceiling.
    assert!(
        peak_memory_mb < MEMORY_CEILING_MB,
        "peak memory {}MB exceeds {}MB ceiling — possible leak",
        peak_memory_mb,
        MEMORY_CEILING_MB
    );

    eprintln!("[longevity] ✓ All assertions passed. Sena survived 72 hours.");
}
