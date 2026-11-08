//! Provider-neutral message model.
//!
//! Each provider maps these types to its own wire format. Blocks a provider
//! does not understand are kept as [`ContentBlock::Opaque`] and sent back
//! unchanged to the same provider (Claude thinking signatures, fallback
//! markers), and dropped by other providers.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMediaType {
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    Png,
}

impl ImageMediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageMediaType::Jpeg => "image/jpeg",
            ImageMediaType::Png => "image/png",
        }
    }
}

/// One piece of a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Base64-encoded image (no `data:` prefix, no newlines).
    Image {
        media_type: ImageMediaType,
        data: String,
    },
    /// The model asks to run a tool.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    /// Result of a tool, sent back in a user message.
    /// `content` holds only `Text` and `Image` blocks.
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
    /// Model reasoning or progress note. Echo back unchanged.
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    /// A provider-specific block kept verbatim (e.g. Claude `redacted_thinking`, `fallback`).
    Opaque {
        provider: String,
        value: Value,
    },
}

impl ContentBlock {
    pub fn text(t: impl Into<String>) -> Self {
        ContentBlock::Text { text: t.into() }
    }

    /// Image from raw bytes (encodes base64).
    pub fn image(media_type: ImageMediaType, bytes: &[u8]) -> Self {
        ContentBlock::Image {
            media_type,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub fn tool_result(id: impl Into<String>, content: Vec<ContentBlock>, is_error: bool) -> Self {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content,
            is_error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content,
        }
    }

    pub fn user_text(t: impl Into<String>) -> Self {
        Self::user(vec![ContentBlock::text(t)])
    }

    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }
}

/// A tool the model may call. `input_schema` is a JSON Schema object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Reasoning effort (Claude `output_config.effort`; OpenAI `reasoning_effort` is not sent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// One completion request.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// `None` = the provider's default model.
    pub model: Option<String>,
    /// Stable system prompt. Keep it byte-identical across calls so it caches.
    pub system: String,
    pub messages: Vec<Message>,
    /// Keep the list and its order stable across calls so it caches.
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    pub effort: Option<Effort>,
    /// Allow several tool calls in one response.
    pub parallel_tool_calls: bool,
    /// Ask the provider to cache the prompt prefix (Anthropic only).
    pub cache: bool,
}

impl Request {
    pub fn new(system: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: None,
            system: system.into(),
            messages,
            tools: Vec::new(),
            max_tokens: 16_000,
            effort: None,
            parallel_tool_calls: true,
            cache: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    /// A safety classifier declined (Claude `refusal`, OpenAI `content_filter`).
    Refusal,
    /// Server-side tool loop paused (Claude); resend to continue.
    PauseTurn,
    Other(String),
}

impl StopReason {
    pub(crate) fn from_anthropic(s: Option<&str>) -> Self {
        match s {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("stop_sequence") => StopReason::StopSequence,
            Some("refusal") => StopReason::Refusal,
            Some("pause_turn") => StopReason::PauseTurn,
            Some(o) => StopReason::Other(o.to_string()),
            None => StopReason::Other("none".into()),
        }
    }
}

/// Token usage of one response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Uncached input tokens (after the last cache breakpoint).
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_creation_input_tokens += o.cache_creation_input_tokens;
        self.cache_read_input_tokens += o.cache_read_input_tokens;
    }

    /// All input tokens, cached or not.
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

/// One completion response.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub id: String,
    /// Model that produced the message (may differ from the request after a fallback).
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
    /// Refusal category or explanation, when `stop_reason` is `Refusal`.
    pub stop_details: Option<String>,
}

impl Response {
    /// All text blocks joined by newlines.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Tool calls in order: (id, name, input).
    pub fn tool_uses(&self) -> Vec<(&str, &str, &Value)> {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input))
                }
                _ => None,
            })
            .collect()
    }
}
