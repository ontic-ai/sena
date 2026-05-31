use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub dependencies: &'static [&'static str],
    pub can_start_without_dependencies: bool,
}

pub const SOUL_ACTOR_ID: &str = "soul";
pub const INFERENCE_ACTOR_ID: &str = "inference";
pub const MEMORY_ACTOR_ID: &str = "memory";
pub const PLATFORM_ACTOR_ID: &str = "platform";
pub const CTP_ACTOR_ID: &str = "ctp";
pub const PROMPT_ACTOR_ID: &str = "prompt";
pub const STT_ACTOR_ID: &str = "stt";
pub const TTS_ACTOR_ID: &str = "tts";
pub const SRI_ACTOR_ID: &str = "sri";

pub const ACTOR_SPECS: &[ActorSpec] = &[
    ActorSpec {
        id: SOUL_ACTOR_ID,
        display_name: "Soul & Identity",
        description: "Persistent personality, session memory, and identity",
        dependencies: &[],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: INFERENCE_ACTOR_ID,
        display_name: "Inference (LLM)",
        description: "Language model - generates all responses",
        dependencies: &[],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: MEMORY_ACTOR_ID,
        display_name: "Memory",
        description: "Stores and retrieves conversation history",
        dependencies: &[INFERENCE_ACTOR_ID],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: PLATFORM_ACTOR_ID,
        display_name: "Platform Sensing",
        description: "Observes active window, clipboard, and keystrokes",
        dependencies: &[],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: CTP_ACTOR_ID,
        display_name: "Thought Processing (CTP)",
        description: "Monitors context and generates proactive thoughts",
        dependencies: &[PLATFORM_ACTOR_ID, INFERENCE_ACTOR_ID],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: PROMPT_ACTOR_ID,
        display_name: "Prompt Assembly",
        description: "Builds context windows before each inference call",
        dependencies: &[INFERENCE_ACTOR_ID],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: STT_ACTOR_ID,
        display_name: "Speech Input (STT)",
        description: "Listens to the microphone and transcribes speech",
        dependencies: &[],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: TTS_ACTOR_ID,
        display_name: "Speech Output (TTS)",
        description: "Synthesizes and plays Sena's voice",
        dependencies: &[],
        can_start_without_dependencies: false,
    },
    ActorSpec {
        id: SRI_ACTOR_ID,
        display_name: "Runtime Interface (SRI)",
        description: "Powers the CLI visualization and signal tracing",
        dependencies: &[],
        can_start_without_dependencies: false,
    },
];

pub fn actor_specs() -> &'static [ActorSpec] {
    ACTOR_SPECS
}

pub fn actor_spec(id: &str) -> Option<&'static ActorSpec> {
    ACTOR_SPECS.iter().find(|spec| spec.id == id)
}

pub fn dependents_of(id: &str) -> Vec<&'static ActorSpec> {
    ACTOR_SPECS
        .iter()
        .filter(|spec| spec.dependencies.contains(&id))
        .collect()
}

pub fn first_missing_dependency(
    selection: &ActorSelection,
    actor_id: &str,
) -> Option<&'static ActorSpec> {
    actor_spec(actor_id).and_then(|spec| {
        spec.dependencies
            .iter()
            .find(|dependency| !selection.contains(dependency))
            .and_then(|dependency| actor_spec(dependency))
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorSelection {
    selected: BTreeSet<&'static str>,
}

impl ActorSelection {
    pub fn all() -> Self {
        Self {
            selected: ACTOR_SPECS.iter().map(|spec| spec.id).collect(),
        }
    }

    pub fn empty() -> Self {
        Self {
            selected: BTreeSet::new(),
        }
    }

    pub fn try_from_ids<I, S>(ids: I) -> Result<Self, ActorSelectionError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut selected = BTreeSet::new();

        for raw_id in ids {
            let actor_id = raw_id.as_ref().trim();
            let spec = actor_spec(actor_id).ok_or_else(|| ActorSelectionError::UnknownActorId {
                actor_id: actor_id.to_string(),
            })?;
            selected.insert(spec.id);
        }

        Ok(Self { selected })
    }

    pub fn contains(&self, actor_id: &str) -> bool {
        self.selected.contains(actor_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.selected.iter().copied()
    }

    pub fn selected_ids(&self) -> Vec<&'static str> {
        self.iter().collect()
    }

    pub fn skipped_ids(&self) -> Vec<&'static str> {
        ACTOR_SPECS
            .iter()
            .map(|spec| spec.id)
            .filter(|actor_id| !self.contains(actor_id))
            .collect()
    }

    pub fn validate(&self) -> Result<(), ActorSelectionError> {
        for spec in ACTOR_SPECS {
            if !self.contains(spec.id) || spec.can_start_without_dependencies {
                continue;
            }

            let missing_dependencies = spec
                .dependencies
                .iter()
                .filter(|dependency| !self.contains(dependency))
                .map(|dependency| (*dependency).to_string())
                .collect::<Vec<_>>();

            if !missing_dependencies.is_empty() {
                return Err(ActorSelectionError::MissingDependencies {
                    actor_id: spec.id.to_string(),
                    missing_dependencies,
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ActorSelectionError {
    #[error("unknown actor id: {actor_id}")]
    UnknownActorId { actor_id: String },

    #[error("actor '{actor_id}' is missing required dependencies: {missing_dependencies:?}")]
    MissingDependencies {
        actor_id: String,
        missing_dependencies: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_requested_actor_graph() {
        assert_eq!(ACTOR_SPECS.len(), 9);
        assert!(actor_spec(SOUL_ACTOR_ID).is_some());
        assert!(actor_spec(INFERENCE_ACTOR_ID).is_some());
        assert!(actor_spec(MEMORY_ACTOR_ID).is_some());
        assert!(actor_spec(PLATFORM_ACTOR_ID).is_some());
        assert!(actor_spec(CTP_ACTOR_ID).is_some());
        assert!(actor_spec(PROMPT_ACTOR_ID).is_some());
        assert!(actor_spec(STT_ACTOR_ID).is_some());
        assert!(actor_spec(TTS_ACTOR_ID).is_some());
        assert!(actor_spec(SRI_ACTOR_ID).is_some());
    }

    #[test]
    fn selection_validation_rejects_missing_dependencies() {
        let selection = ActorSelection::try_from_ids([CTP_ACTOR_ID, INFERENCE_ACTOR_ID])
            .expect("selection should parse");

        let error = selection.validate().expect_err("selection should be invalid");
        assert_eq!(
            error,
            ActorSelectionError::MissingDependencies {
                actor_id: CTP_ACTOR_ID.to_string(),
                missing_dependencies: vec![PLATFORM_ACTOR_ID.to_string()],
            }
        );
    }

    #[test]
    fn dependents_lookup_matches_graph() {
        let dependents = dependents_of(INFERENCE_ACTOR_ID)
            .into_iter()
            .map(|spec| spec.id)
            .collect::<Vec<_>>();

        assert_eq!(
            dependents,
            vec![MEMORY_ACTOR_ID, CTP_ACTOR_ID, PROMPT_ACTOR_ID]
        );
    }

    #[test]
    fn skipped_ids_include_unselected_actors() {
        let selection =
            ActorSelection::try_from_ids([SOUL_ACTOR_ID, INFERENCE_ACTOR_ID]).unwrap();

        assert_eq!(
            selection.skipped_ids(),
            vec![
                MEMORY_ACTOR_ID,
                PLATFORM_ACTOR_ID,
                CTP_ACTOR_ID,
                PROMPT_ACTOR_ID,
                STT_ACTOR_ID,
                TTS_ACTOR_ID,
                SRI_ACTOR_ID,
            ]
        );
    }

    #[test]
    fn prompt_selection_is_valid_without_soul() {
        let selection =
            ActorSelection::try_from_ids([INFERENCE_ACTOR_ID, PROMPT_ACTOR_ID]).unwrap();

        selection
            .validate()
            .expect("prompt should only require inference");
    }
}