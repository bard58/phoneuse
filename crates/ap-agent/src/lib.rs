//! The agentphone agent loop.
//!
//! observe (downscaled screenshot) -> LLM with tools -> act on a
//! `Box<dyn Device>` -> frame strip -> repeat, until the model calls `done`,
//! the step limit is reached, or the host cancels (take-over).
//!
//! - [`Agent`]: the loop. Build it with a [`Provider`](ap_llm::Provider), a
//!   device, an [`AgentHost`] and [`AgentOptions`].
//! - [`AgentHost`]: callbacks for CLI / MCP front ends: step events,
//!   confirmations, questions, unlock.
//! - [`CancelToken`]: stop the run at once ("the user took over").
//! - [`Observer`]: how the screen is captured; [`ScreenObserver`] uses
//!   `ap_screen::settle` and `ap_screen::frame_strip`.
//! - [`tools`]: the tool set and its coordinate convention (fractions 0..1).
//! - [`UsageLog`]: tokens and estimated cost per step.
//!
//! Safety rules (from upstream phoneuse): the model must call `confirm`
//! and the host must approve before any action marked `irreversible`;
//! unlocking needs `allow_unlock` *and* the owner's approval, and the
//! passcode never reaches the model; the run stops at `max_steps`.

mod agent;
mod cancel;
mod host;
mod observe;
pub mod prompt;
pub mod tools;
mod usage;

#[cfg(test)]
mod tests;

pub use agent::{Agent, AgentOptions, Outcome, RunReport};
pub use ap_core;
pub use ap_llm;
pub use cancel::CancelToken;
pub use host::{AgentEvent, AgentHost, ConfirmKind, ConfirmRequest, NoopHost, UnlockResult};
pub use observe::{Observation, Observer, ScreenObserver};
pub use usage::{StepUsage, UsageLog};
