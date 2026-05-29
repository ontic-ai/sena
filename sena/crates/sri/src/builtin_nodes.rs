use crate::{FunctionStub, HealthStatus, SriNode};

// TODO - move each node definition to its owning crate when that crate implements SriNode.

#[derive(Clone, Copy)]
struct BuiltinNode {
    shelf_path: &'static str,
    display_name: &'static str,
    description: &'static str,
    function_stubs: &'static [(&'static str, &'static str)],
}

impl SriNode for BuiltinNode {
    fn shelf_path(&self) -> &str {
        self.shelf_path
    }

    fn display_name(&self) -> &str {
        self.display_name
    }

    fn description(&self) -> &str {
        self.description
    }

    fn function_stubs(&self) -> Vec<FunctionStub> {
        self.function_stubs
            .iter()
            .map(|(name, description)| FunctionStub::unavailable(*name, *description))
            .collect()
    }

    fn health_status(&self) -> HealthStatus {
        HealthStatus::Unavailable
    }
}

pub fn scaffold_builtin_nodes() -> Vec<Box<dyn SriNode>> {
    vec![
        Box::new(BuiltinNode {
            shelf_path: "identity.soul",
            display_name: "soul",
            description: "tone, style, and identity signals",
            function_stubs: &[
                (
                    "adapt_personality",
                    "apply live warmth and verbosity changes",
                ),
                ("summarize_identity", "condense learned identity traits"),
            ],
        }),
        Box::new(BuiltinNode {
            shelf_path: "perception.hearing",
            display_name: "hearing",
            description: "speech capture and transcription",
            function_stubs: &[
                ("transcribe_audio", "stream microphone audio into text"),
                ("detect_wakeword", "watch for wakeword activations"),
            ],
        }),
        Box::new(BuiltinNode {
            shelf_path: "perception.sight",
            display_name: "sight",
            description: "visual context intake placeholder",
            function_stubs: &[
                (
                    "capture_context",
                    "collect visual context from the active window",
                ),
                ("analyze_scene", "derive scene semantics from visual input"),
            ],
        }),
        Box::new(BuiltinNode {
            shelf_path: "cognition.memory",
            display_name: "memory",
            description: "episodic and semantic retrieval",
            function_stubs: &[
                ("recall_relevant", "retrieve relevant long-term memories"),
                ("store_memory", "persist a new memory chunk"),
            ],
        }),
        Box::new(BuiltinNode {
            shelf_path: "cognition.thought",
            display_name: "thought",
            description: "context assembly and reasoning orchestration",
            function_stubs: &[
                ("assemble_context", "assemble context from active signals"),
                ("trigger_reasoning", "trigger a proactive reasoning pass"),
            ],
        }),
        Box::new(BuiltinNode {
            shelf_path: "expression.voice",
            display_name: "voice",
            description: "speech synthesis output",
            function_stubs: &[
                ("speak_text", "render text through TTS"),
                ("queue_utterance", "schedule voice playback"),
            ],
        }),
        Box::new(BuiltinNode {
            shelf_path: "expression.language",
            display_name: "language",
            description: "language generation and response streaming",
            function_stubs: &[
                ("generate_response", "generate a language response"),
                ("stream_sentence", "stream the next sentence to output"),
            ],
        }),
        Box::new(BuiltinNode {
            shelf_path: "environment.system",
            display_name: "system",
            description: "runtime health and host resources",
            function_stubs: &[
                ("read_resources", "sample CPU, RAM, and VRAM usage"),
                ("watch_runtime", "watch runtime lifecycle changes"),
            ],
        }),
    ]
}
