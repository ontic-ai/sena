use std::collections::BTreeMap;

use bus::{
    CancelRequest, ParsedModelOutput, SignalMessage, TaskRequest, SENA_CANCEL_TAG, SENA_OUTPUT_ROOT,
    SENA_REPLY_TAG, SENA_SIGNAL_TAG, SENA_TASK_TAG, SENA_THINK_TAG,
};
use roxmltree::{Document, Node};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OutputParseError {
    #[error("failed to parse xml: {0}")]
    Xml(String),
    #[error("expected root element `{expected}` but found `{actual}`")]
    InvalidRoot { expected: &'static str, actual: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolMessage {
    Parsed(ParsedModelOutput),
    Fallback {
        raw: String,
        reply: String,
        reason: String,
    },
}

impl ProtocolMessage {
    pub fn spoken_reply(&self) -> Option<&str> {
        match self {
            Self::Parsed(output) => output.spoken_reply(),
            Self::Fallback { reply, .. } if !reply.is_empty() => Some(reply.as_str()),
            Self::Fallback { .. } => None,
        }
    }
}

pub fn interpret_model_output(raw: &str) -> ProtocolMessage {
    match parse_output(raw) {
        Ok(output) => ProtocolMessage::Parsed(output),
        Err(reason) => ProtocolMessage::Fallback {
            raw: raw.to_owned(),
            reply: normalize_text(raw),
            reason: reason.to_string(),
        },
    }
}

pub fn parse_output(raw: &str) -> Result<ParsedModelOutput, OutputParseError> {
    let document = Document::parse(raw).map_err(|err| OutputParseError::Xml(err.to_string()))?;
    let root = document.root_element();
    if root.tag_name().name() != SENA_OUTPUT_ROOT {
        return Err(OutputParseError::InvalidRoot {
            expected: SENA_OUTPUT_ROOT,
            actual: root.tag_name().name().to_owned(),
        });
    }

    let mut output = ParsedModelOutput {
        raw: raw.to_owned(),
        reply: None,
        think: Vec::new(),
        tasks: Vec::new(),
        cancels: Vec::new(),
        signals: Vec::new(),
        unknown_tags: Vec::new(),
    };

    for child in root.children().filter(Node::is_element) {
        match child.tag_name().name() {
            SENA_REPLY_TAG => append_reply(&mut output, node_text(child)),
            SENA_THINK_TAG => {
                let think = node_text(child);
                if !think.is_empty() {
                    output.think.push(think);
                }
            }
            SENA_TASK_TAG => output.tasks.push(parse_task(child)),
            SENA_CANCEL_TAG => output.cancels.push(parse_cancel(child)),
            SENA_SIGNAL_TAG => output.signals.push(parse_signal(child)),
            other => output.unknown_tags.push(other.to_owned()),
        }
    }

    Ok(output)
}

fn append_reply(output: &mut ParsedModelOutput, reply: String) {
    if reply.is_empty() {
        return;
    }

    match &mut output.reply {
        Some(existing) => {
            existing.push('\n');
            existing.push_str(&reply);
        }
        None => output.reply = Some(reply),
    }
}

fn parse_task(node: Node<'_, '_>) -> TaskRequest {
    let mut args = collect_attributes(node, &["mode", "capability", "run_id"]);
    for child in node.children().filter(Node::is_element) {
        let value = node_text(child);
        if !value.is_empty() {
            args.insert(child.tag_name().name().to_owned(), value);
        }
    }

    TaskRequest {
        mode: node.attribute("mode").map(str::to_owned),
        capability: node
            .attribute("capability")
            .map(str::to_owned)
            .or_else(|| args.get("capability").cloned())
            .unwrap_or_else(|| "actions.unknown".to_owned()),
        run_id: node.attribute("run_id").map(str::to_owned),
        args,
    }
}

fn parse_cancel(node: Node<'_, '_>) -> CancelRequest {
    CancelRequest {
        run_id: node.attribute("run_id").map(str::to_owned),
        scope: node.attribute("scope").map(str::to_owned),
        reason: match node_text(node) {
            text if text.is_empty() => None,
            text => Some(text),
        },
    }
}

fn parse_signal(node: Node<'_, '_>) -> SignalMessage {
    SignalMessage {
        name: node.attribute("name").map(str::to_owned),
        value: node_text(node),
        attributes: collect_attributes(node, &["name"]),
    }
}

fn collect_attributes(node: Node<'_, '_>, skip: &[&str]) -> BTreeMap<String, String> {
    node.attributes()
        .filter(|attribute| !skip.contains(&attribute.name()))
        .map(|attribute| (attribute.name().to_owned(), attribute.value().to_owned()))
        .collect()
}

fn node_text(node: Node<'_, '_>) -> String {
    let text = node
        .descendants()
        .filter(|candidate| candidate.is_text())
        .filter_map(|candidate| candidate.text())
        .collect::<Vec<_>>()
        .join(" ");
    normalize_text(&text)
}

fn normalize_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{interpret_model_output, parse_output, ProtocolMessage};

    #[test]
    fn parses_mixed_sena_output() {
        let raw = r#"
            <sena_output>
                <sena_think>quiet reasoning</sena_think>
                <sena_reply>Hello there.</sena_reply>
                <sena_task capability="actions.timer" mode="background" run_id="task-1">
                    <seconds>30</seconds>
                </sena_task>
            </sena_output>
        "#;

        let parsed = parse_output(raw).expect("expected valid xml");

        assert_eq!(parsed.reply.as_deref(), Some("Hello there."));
        assert_eq!(parsed.think, vec!["quiet reasoning"]);
        assert_eq!(parsed.tasks.len(), 1);
        assert_eq!(parsed.tasks[0].capability, "actions.timer");
        assert_eq!(parsed.tasks[0].args.get("seconds").map(String::as_str), Some("30"));
    }

    #[test]
    fn falls_back_on_malformed_output() {
        let message = interpret_model_output("hello from fallback mode");

        match message {
            ProtocolMessage::Fallback { reply, .. } => {
                assert_eq!(reply, "hello from fallback mode");
            }
            ProtocolMessage::Parsed(_) => panic!("expected fallback"),
        }
    }

    #[test]
    fn spoken_reply_never_includes_thought_text() {
        let raw = r#"
            <sena_output>
                <sena_think>do not say this aloud</sena_think>
                <sena_reply>Only this line should be spoken.</sena_reply>
            </sena_output>
        "#;

        let parsed = parse_output(raw).expect("expected valid xml");
        let spoken = parsed.spoken_reply().expect("reply should be present");

        assert_eq!(spoken, "Only this line should be spoken.");
        assert!(!spoken.contains("do not say this aloud"));
    }
}
