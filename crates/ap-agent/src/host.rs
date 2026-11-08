//! Host callbacks: how a front end (CLI, MCP, GUI) talks to a running agent.

use ap_core::Device;
use ap_llm::Usage;
use serde::Serialize;
use serde_json::Value;

use crate::agent::Outcome;

/// Something that happened in the run. Sent to [`AgentHost::on_step`].
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    Started {
        task: String,
        device: String,
        model: String,
    },
    /// Text the model wrote (its plan or a progress note).
    ModelText {
        step: u32,
        text: String,
    },
    ToolCall {
        step: u32,
        name: String,
        input: Value,
    },
    ToolResult {
        step: u32,
        name: String,
        ok: bool,
        /// Text part of the result (no image).
        summary: String,
    },
    /// Tokens and estimated cost after each LLM call.
    Usage {
        step: u32,
        model: String,
        usage: Usage,
        step_cost_usd: Option<f64>,
        total: Usage,
        total_cost_usd: Option<f64>,
    },
    Finished {
        outcome: Outcome,
    },
}

/// What the agent asks the owner to approve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmKind {
    /// Sending, paying, buying, deleting, posting, changing settings ...
    Irreversible,
    /// Unlocking the phone with the saved passcode.
    Unlock,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfirmRequest {
    pub kind: ConfirmKind,
    /// One line: what will happen ("Send the message 'On my way' to Alex").
    pub action: String,
    /// Optional extra detail from the model.
    pub details: Option<String>,
}

/// Result of [`AgentHost::unlock`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnlockResult {
    Unlocked,
    NotLocked,
    /// The passcode did not work; the host must not retry.
    StillLocked,
    /// The host cannot unlock (no saved passcode, no support, ...).
    Unavailable(String),
}

/// Callbacks from the agent loop. Only `confirm` and `ask_user` must be answered;
/// the defaults deny and stay silent, which is safe.
#[async_trait::async_trait]
pub trait AgentHost: Send + Sync {
    /// Progress events. Keep it fast; do not block.
    fn on_step(&self, _event: &AgentEvent) {}

    /// Ask the owner to approve an action. `true` = approved.
    async fn confirm(&self, _request: &ConfirmRequest) -> bool {
        false
    }

    /// Ask the owner a question; return their answer (empty = no answer).
    async fn ask_user(&self, _question: &str) -> String {
        String::new()
    }

    /// Enter the owner's saved passcode on the device, once. Called only after
    /// `allow_unlock` is on and [`confirm`](Self::confirm) approved a
    /// [`ConfirmKind::Unlock`] request. The passcode stays inside the host:
    /// never return it, log it, or put it in an event.
    async fn unlock(&self, _device: &dyn Device) -> UnlockResult {
        UnlockResult::Unavailable("this host cannot unlock the phone".into())
    }
}

/// A host that approves nothing and answers nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHost;

impl AgentHost for NoopHost {}
