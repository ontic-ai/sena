//! M2.7 — End-to-end inference loop integration test.
//!
//! Wires: Manual InferenceRequested → Inference → Response on bus (logged to Soul + Memory)

use std::sync::Arc;
use std::time::Duration;

use bus::{Actor, Event, EventBus, InferenceEvent};
use tempfile::tempdir;

/// Create the minimal Ollama manifest directory structure so that
/// `discovery::discover_models()` finds exactly one model.
fn create_mock_ollama_structure(model_dir: &std::path::Path) {
    let manifests_lib = model_dir
        .join("manifests")
        .join("registry.ollama.ai")
        .join("library");
    std::fs::create_dir_all(&manifests_lib).expect("create manifests dir");

    let test_model_dir = manifests_lib.join("test-model");
    std::fs::create_dir_all(&test_model_dir).expect("create model dir");

    let manifest_json = r#"{
  "schemaVersion": 2,
  "layers": [
    {
      "mediaType": "application/vnd.ollama.image.model",
      "digest": "sha256:testdigest123",
      "size": 3000000000
    }
  ]
}"#;
    std::fs::write(test_model_dir.join("latest"), manifest_json).expect("write manifest");

    let blobs_dir = model_dir.join("blobs");
    std::fs::create_dir_all(&blobs_dir).expect("create blobs dir");
    std::fs::write(blobs_dir.join("sha256-testdigest123"), vec![0u8; 64]).expect("write blob stub");
}

/// M2.7 integration test: verify event flow from InferenceRequested → InferenceCompleted.
///
/// This does NOT load a real GGUF model (MockBackend returns placeholder text).
/// This DOES verify:
/// - InferenceRequested is sent to inference actor
/// - InferenceCompleted is broadcast on the bus
/// - Memory and Soul actors are running (they will process the response)
#[tokio::test]
async fn end_to_end_thought_triggers_inference_cycle() {
    let dir = tempdir().expect("tempdir");
    let soul_path = dir.path().join("soul.redb.enc");
    let graph_path = dir.path().join("graph.redb.enc");
    let vector_path = dir.path().join("vector.usearch.enc");

    let bus = Arc::new(EventBus::new());
    let master_key = crypto::MasterKey::from_bytes([0u8; 32]);

    // Initialize Soul
    let soul_actor = soul::SoulActor::new(&soul_path, master_key);
    let mut soul_box = Box::new(soul_actor);
    soul_box.start(Arc::clone(&bus)).await.expect("soul start");
    let soul_handle = tokio::spawn(async move {
        let _ = soul_box.run().await;
        soul_box.stop().await.expect("soul stop");
    });

    // Initialize Memory (ech0 placeholder)
    let memory_actor = memory::MemoryActor::new(&graph_path, &vector_path);
    let mut memory_box = Box::new(memory_actor);
    memory_box
        .start(Arc::clone(&bus))
        .await
        .expect("memory start");
    let memory_handle = tokio::spawn(async move {
        let _ = memory_box.run().await;
        memory_box.stop().await.expect("memory stop");
    });

    // Initialize Inference (MockBackend — returns placeholder text)
    let model_dir = dir.path().join("models");
    std::fs::create_dir_all(&model_dir).expect("create model_dir");
    create_mock_ollama_structure(&model_dir);

    let inference_actor = inference::InferenceActor::new(
        model_dir,
        Box::new(inference::MockBackend::new()), // Mock backend
        inference::BackendType::Cpu,
    );
    let mut inference_box = Box::new(inference_actor);
    inference_box
        .start(Arc::clone(&bus))
        .await
        .expect("inference start");
    let inference_handle = tokio::spawn(async move {
        let _ = inference_box.run().await;
        inference_box.stop().await.expect("inference stop");
    });

    // Subscribe to events
    let mut rx = bus.subscribe_broadcast();

    // Act: Manually emit InferenceRequested (simulates prompt composer → inference flow)
    let prompt = "Test prompt: what is the meaning of life?".to_string();
    bus.send_directed(
        "inference",
        Event::Inference(InferenceEvent::InferenceRequested {
            prompt,
            priority: bus::events::inference::Priority::Normal,
            request_id: 1,
        }),
    )
    .await
    .expect("send InferenceRequested");

    // Wait for InferenceCompleted on the bus (timeout after 5 seconds)
    let mut received_inference_response = false;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        tokio::select! {
            msg = rx.recv() => {
                if let Ok(Event::Inference(InferenceEvent::InferenceCompleted { text, request_id, token_count })) = msg {
                    received_inference_response = true;
                    eprintln!("[test] InferenceCompleted received: request_id={}, tokens={}, text_len={}",
                              request_id, token_count, text.len());

                    // Verify response structure
                    assert!(text.len() > 0, "response text should not be empty");
                    assert_eq!(request_id, 1, "request_id should match");
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    // Shutdown
    bus.broadcast(Event::System(bus::SystemEvent::ShutdownSignal))
        .await
        .expect("shutdown broadcast");

    tokio::time::sleep(Duration::from_millis(200)).await;

    soul_handle.abort();
    memory_handle.abort();
    inference_handle.abort();

    // Assert
    assert!(
        received_inference_response,
        "InferenceCompleted was not received within timeout — inference loop did not complete"
    );
}
