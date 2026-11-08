//! Core types and traits shared by every agentphone crate.
//!
//! - [`geometry`]: normalized [`Point`] (0..1) and [`Size`].
//! - [`frame`]: captured screen [`Frame`].
//! - [`input`]: [`Button`], [`Key`], [`Modifiers`], [`SwipeDirection`], [`Gesture`].
//! - [`device`]: [`Device`] and [`DeviceProvider`] traits, [`DeviceInfo`], [`Capabilities`].
//! - [`error`]: [`Error`] and [`Result`].
//! - [`config`]: user [`Config`] stored in the platform config directory.
//! - `mock`: a recording `MockDevice` for tests (feature `mock`, on by default).

pub mod config;
pub mod device;
pub mod error;
pub mod frame;
pub mod geometry;
pub mod input;
#[cfg(feature = "mock")]
pub mod mock;

pub use async_trait::async_trait;
pub use config::{AndroidConfig, Config, IosConfig, LlmConfig, SafetyConfig};
pub use device::{
    Capabilities, Device, DeviceInfo, DeviceProvider, IOS_TOUCH_MIN_MAJOR, IOS_UPDATE_HINT,
    Platform, ios_touch_error, ios_touch_message, ios_touch_problem, touch_unavailable,
};
pub use error::{Error, Result};
pub use frame::Frame;
pub use geometry::{Point, Size};
pub use input::{Button, Gesture, Key, Modifiers, SwipeDirection};
