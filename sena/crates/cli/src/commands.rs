#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FixedArgumentSpec {
    pub(crate) value: &'static str,
    pub(crate) description: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandArgumentKind {
    None,
    FreeText,
    FixedList(&'static [FixedArgumentSpec]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpGroup {
    Navigation,
    VoiceAndInference,
    System,
    Memory,
    Debug,
    Other,
}

impl HelpGroup {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::VoiceAndInference => "Voice & Inference",
            Self::System => "System",
            Self::Memory => "Memory",
            Self::Debug => "Debug",
            Self::Other => "Other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    pub(crate) command: &'static str,
    pub(crate) description: &'static str,
    pub(crate) help_group: HelpGroup,
    pub(crate) argument_kind: CommandArgumentKind,
}

impl CommandSpec {
    pub(crate) fn fixed_arguments(self) -> Option<&'static [FixedArgumentSpec]> {
        match self.argument_kind {
            CommandArgumentKind::FixedList(arguments) => Some(arguments),
            CommandArgumentKind::None | CommandArgumentKind::FreeText => None,
        }
    }
}

pub(crate) const TAB_ARGUMENTS: &[FixedArgumentSpec] = &[
    FixedArgumentSpec {
        value: "diag",
        description: "Inference diagnostics",
    },
    FixedArgumentSpec {
        value: "config",
        description: "Configuration editor",
    },
    FixedArgumentSpec {
        value: "actors",
        description: "Actor selection",
    },
    FixedArgumentSpec {
        value: "resources",
        description: "Resource monitor",
    },
];

pub(crate) const DEBUG_ARGUMENTS: &[FixedArgumentSpec] = &[
    FixedArgumentSpec {
        value: "inference",
        description: "Inference tracing",
    },
    FixedArgumentSpec {
        value: "speech",
        description: "Speech tracing",
    },
    FixedArgumentSpec {
        value: "memory",
        description: "Memory tracing",
    },
    FixedArgumentSpec {
        value: "ctp",
        description: "CTP tracing",
    },
    FixedArgumentSpec {
        value: "soul",
        description: "Soul tracing",
    },
    FixedArgumentSpec {
        value: "platform",
        description: "Platform tracing",
    },
    FixedArgumentSpec {
        value: "sri",
        description: "SRI tracing",
    },
];

pub(crate) const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        command: "/tab",
        description: "Open a tab window",
        help_group: HelpGroup::Navigation,
        argument_kind: CommandArgumentKind::FixedList(TAB_ARGUMENTS),
    },
    CommandSpec {
        command: "/tree",
        description: "Toggle full tree view vs live navigation view",
        help_group: HelpGroup::Navigation,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/sri",
        description: "Print full SRI node snapshot to signals panel",
        help_group: HelpGroup::Navigation,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/say",
        description: "Speak text verbatim through TTS (audio test)",
        help_group: HelpGroup::VoiceAndInference,
        argument_kind: CommandArgumentKind::FreeText,
    },
    CommandSpec {
        command: "/run",
        description: "Run full inference pipeline as if spoken",
        help_group: HelpGroup::VoiceAndInference,
        argument_kind: CommandArgumentKind::FreeText,
    },
    CommandSpec {
        command: "/load",
        description: "Load a GGUF model from the given path",
        help_group: HelpGroup::VoiceAndInference,
        argument_kind: CommandArgumentKind::FreeText,
    },
    CommandSpec {
        command: "/listen",
        description: "Enable voice input routing to inference",
        help_group: HelpGroup::VoiceAndInference,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/stop",
        description: "Disable voice input routing (mic stays open)",
        help_group: HelpGroup::VoiceAndInference,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/status",
        description: "Show all actor health statuses",
        help_group: HelpGroup::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/ping",
        description: "Show daemon uptime",
        help_group: HelpGroup::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/shutdown",
        description: "Gracefully shut down the Sena daemon",
        help_group: HelpGroup::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/test-mode",
        description: "Restart the daemon into actor selection mode",
        help_group: HelpGroup::System,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/memory",
        description: "Show remembered user context",
        help_group: HelpGroup::Memory,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/query",
        description: "Query memory for relevant nodes",
        help_group: HelpGroup::Memory,
        argument_kind: CommandArgumentKind::FreeText,
    },
    CommandSpec {
        command: "/memory-clear",
        description: "Clear persistent memory contents",
        help_group: HelpGroup::Memory,
        argument_kind: CommandArgumentKind::None,
    },
    CommandSpec {
        command: "/debug",
        description: "Choose a subsystem for verbose tracing hints",
        help_group: HelpGroup::Debug,
        argument_kind: CommandArgumentKind::FixedList(DEBUG_ARGUMENTS),
    },
    CommandSpec {
        command: "/help",
        description: "Show this screen",
        help_group: HelpGroup::Other,
        argument_kind: CommandArgumentKind::None,
    },
];

pub(crate) const HELP_LEFT_COLUMN_GROUPS: &[HelpGroup] = &[
    HelpGroup::Navigation,
    HelpGroup::VoiceAndInference,
    HelpGroup::System,
    HelpGroup::Other,
];

pub(crate) const HELP_RIGHT_COLUMN_GROUPS: &[HelpGroup] = &[HelpGroup::Memory, HelpGroup::Debug];

pub(crate) fn find_command(command: &str) -> Option<(usize, &'static CommandSpec)> {
    COMMANDS
        .iter()
        .enumerate()
        .find(|(_, spec)| spec.command == command)
}

pub(crate) fn find_fixed_argument(
    command: &str,
    value: &str,
) -> Option<&'static FixedArgumentSpec> {
    let (_, spec) = find_command(command)?;
    spec.fixed_arguments()?
        .iter()
        .find(|argument| argument.value == value)
}

pub(crate) fn commands_in_group(group: HelpGroup) -> impl Iterator<Item = &'static CommandSpec> {
    COMMANDS
        .iter()
        .filter(move |command| command.help_group == group)
}

#[cfg(test)]
mod tests {
    use super::{CommandArgumentKind, COMMANDS, TAB_ARGUMENTS, find_command};

    #[test]
    fn tab_command_uses_fixed_tab_argument_list() {
        let (_, tab) = find_command("/tab").expect("/tab should be registered");

        assert_eq!(tab.description, "Open a tab window");
        assert_eq!(tab.argument_kind, CommandArgumentKind::FixedList(TAB_ARGUMENTS));
    }

    #[test]
    fn run_say_query_and_load_are_free_text_commands() {
        for command_name in ["/run", "/say", "/query", "/load"] {
            let (_, command) = find_command(command_name).expect("command should be registered");
            assert_eq!(command.argument_kind, CommandArgumentKind::FreeText);
        }

        assert!(COMMANDS.iter().any(|command| command.command == "/debug"));
    }
}