//! Conversation messages and content blocks.

use serde::{Deserialize, Serialize};

use super::tool::ToolCall;

/// The role of a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// System-level instructions.
    System,
    /// A user message.
    User,
    /// An assistant (model) message.
    Assistant,
    /// A tool result message.
    Tool,
}

/// A single content block within a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentBlock {
    /// Plain text content.
    Text(String),
    /// An extended-thinking block.
    ///
    /// `signature` binds the text for providers that verify it (Anthropic):
    /// the block must be replayed verbatim — never anonymized — for the model
    /// to accept the follow-up turn.
    Thinking {
        /// The reasoning text.
        text: String,
        /// Provider signature over `text`, when the provider issues one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// A provider-redacted thinking payload, replayed verbatim.
    RedactedThinking {
        /// Opaque provider payload.
        data: String,
    },
}

/// Persisted provenance is separate from the provider-compatible wire role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    #[default]
    Unspecified,
    HostEvent,
}

/// A single conversation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// The role of the message.
    pub role: Role,
    /// Host provenance; old snapshots retain their unspecified origin.
    #[serde(default)]
    pub origin: MessageOrigin,
    /// The content blocks of the message.
    pub content: Vec<ContentBlock>,
    /// Tool calls made by an assistant message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// The tool call id this message is a result for (tool role only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Creates a text message with the given role.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            origin: MessageOrigin::Unspecified,
            content: vec![ContentBlock::Text(text.into())],
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Construct host data without representing it as a new human instruction.
    pub fn host_event(text: impl Into<String>) -> Self {
        let mut message = Self::text(Role::User, text);
        message.origin = MessageOrigin::HostEvent;
        message
    }

    /// Returns the concatenated **text** content of the message.
    ///
    /// Thinking blocks are excluded on purpose: this is what user-facing
    /// histories, compression transcripts and audit previews consume, and
    /// reasoning text does not belong in any of them.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .map(|block| match block {
                ContentBlock::Text(t) => t.as_str(),
                ContentBlock::Thinking {
                    ..
                }
                | ContentBlock::RedactedThinking {
                    ..
                } => "",
            })
            .collect()
    }

    /// Returns the concatenated thinking text, if any.
    pub fn thinking_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Thinking {
                    text,
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether the message carries any thinking block.
    pub fn has_thinking(&self) -> bool {
        self.content.iter().any(|block| {
            matches!(block, ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. })
        })
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;
    #[test]
    fn host_origin_round_trips_and_legacy_defaults() {
        let event=Message::host_event("data, not permission");
        let restored:Message=serde_json::from_slice(&serde_json::to_vec(&event).unwrap()).unwrap();
        assert_eq!(restored.origin,MessageOrigin::HostEvent);assert_eq!(restored.role,Role::User);
        let mut legacy=serde_json::to_value(Message::text(Role::User,"hello")).unwrap();legacy.as_object_mut().unwrap().remove("origin");
        assert_eq!(serde_json::from_value::<Message>(legacy).unwrap().origin,MessageOrigin::Unspecified);
    }
}
