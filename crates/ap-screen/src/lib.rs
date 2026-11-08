//! Screen helpers for agentphone. Nothing here is device specific: everything
//! works on any [`ap_core::Device`].
//!
//! - [`detect`]: change detection on a coarse luma thumbnail (port of phoneuse
//!   `frames.py`), video-aware: regions that keep moving are ignored.
//! - [`settle`](mod@settle): wait until the screen is still ([`settle()`], [`Settler`]).
//! - [`strip`]: frames right after an action ([`frame_strip`]), de-duplicated.
//! - [`compose`]: put a strip side by side into one image.
//! - [`llm`]: downscale and JPEG-encode a frame for a model.
//! - [`overlay`]: marker and coordinate grid for debugging and grounding.
//! - [`font`]: a tiny 5x7 bitmap font used for labels.

pub mod compose;
pub mod detect;
pub mod font;
pub mod llm;
pub mod overlay;
pub mod settle;
pub mod strip;

pub use ap_core;
pub use compose::{StripLayout, compose_strip};
pub use detect::{Change, ChangeDetector, SettleConfig, Thumb};
pub use llm::{LlmImage, LlmImageOptions, encode_for_llm, resize_max_side};
pub use overlay::{GridOptions, MarkerStyle, draw_grid, draw_marker};
pub use settle::{Settled, Settler, settle, settle_with};
pub use strip::{
    DEFAULT_STRIP_OFFSETS, StripDedup, dedup_strip, frame_strip, frame_strip_timed, strip_legend,
};
