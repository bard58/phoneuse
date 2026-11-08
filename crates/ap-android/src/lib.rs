//! Android backend over adb: screencap, input, am start, UI dump.
//!
//! The crate talks to the local adb server (`adb start-server`, TCP 5037) with
//! a small async client ([`adb::AdbServer`]). Every device operation goes
//! through the [`Transport`] trait, so tests can use a fake transport.
//!
//! - [`AndroidProvider`]: finds devices (USB and Wi-Fi), connects, pairs.
//! - [`AndroidDevice`]: implements [`ap_core::Device`] with `input`,
//!   `screencap`, `monkey`, `pm` and `uiautomator`.

pub mod adb;
pub mod device;
pub mod locate;
pub mod parse;
pub mod provider;

pub use adb::{AdbServer, Transport};
pub use ap_core;
pub use device::AndroidDevice;
pub use locate::locate_adb;
pub use parse::{DeviceEntry, DeviceState};
pub use provider::AndroidProvider;
