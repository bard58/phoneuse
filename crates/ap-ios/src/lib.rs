//! iOS backend for agentphone: drive an iPhone from Windows, macOS or Linux
//! with no Mac, through idevice 0.1.68.
//!
//! Supported: iOS 17.4 and later (Route B: CoreDeviceProxy tunnel with the
//! jktcp userspace TCP stack, CoreDevice HID input, DVT screenshot). Below
//! iOS 17.4, discovery lists the device and `connect` returns a setup error.
//!
//! - [`IosProvider`]: a [`DeviceProvider`](ap_core::DeviceProvider). Finds
//!   devices over usbmuxd (Windows: Apple Mobile Device Service on
//!   127.0.0.1:27015) and saved Wi-Fi pairings (mDNS `_apple-mobdev2._tcp`,
//!   `_remotepairing._tcp`).
//! - [`IosDevice`]: a [`Device`](ap_core::Device). [`CancelHandle`] (from
//!   [`IosDevice::cancel_handle`]) stops its in-flight calls.
//! - [`setup`]: checks and fix actions for `agentphone doctor`.
//! - [`runtime_builder`], [`block_on`]: big-stack runtime helpers (Windows).
//! - [`models`]: iPhone model table. [`keymap`], [`coords`]: input mapping.
//!
//! Status: compile-tested only. No device was attached during development.
//! Every device path is UNVERIFIED. See `README.md`.

pub mod backend;
mod cancel;
mod connect;
pub mod coords;
mod device;
mod errors;
mod hid;
pub mod keymap;
pub mod mdns;
pub mod models;
mod provider;
mod runtime;
pub mod setup;
pub mod store;
mod tunnel;
pub mod version;
mod worker;

pub use ap_core;
pub use cancel::{CANCELLED_PREFIX, CancelCheck, CancelHandle, cancelled, is_cancelled};
pub use connect::usbmuxd_hint;
pub use device::{IosDevice, SCROLL_DURATION};
pub use provider::IosProvider;
pub use runtime::{MAIN_STACK, WORKER_STACK, block_on, runtime_builder};
pub use tunnel::REQUIRED_SERVICES;
pub use version::{IosVersion, Route, UNSUPPORTED_IOS_HINT};
