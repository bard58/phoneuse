//! Route A: iPhone input through an external HID dongle.
//!
//! An ESP32-S3 (BLE HID) or an RP2040/ESP32-S3 (USB HID through the
//! Lightning/USB-C camera adapter) acts as a keyboard, a relative mouse, and an
//! absolute pointer. iOS AssistiveTouch turns the pointer into touches. The PC
//! drives the dongle over USB serial. Works on any iPhone with AssistiveTouch
//! (iOS 13+), with no Developer Mode and nothing installed on the phone.
//!
//! - [`report`]: the HID report map and report bytes (same as upstream phoneuse).
//! - [`calibration`]: per-model drag tables ([`CalibrationProfile`]).
//! - [`gesture`]: the gesture model as pure step plans.
//! - [`protocol`]: the framed serial protocol (COBS + CRC-16, versioned).
//! - [`transport`]: [`Transport`] trait, [`SerialTransport`], [`MemoryTransport`], [`Link`].
//! - [`device`]: [`HidDevice`] (ap-core [`Device`](ap_core::Device)), [`ScreenSource`], [`HidProvider`].
//! - [`discover`](mod@discover): find candidate serial ports.
//!
//! The firmware is in the repository's `firmware/` folder.

pub mod calibration;
pub mod device;
pub mod discover;
pub mod gesture;
pub mod protocol;
pub mod report;
pub mod transport;

pub use ap_core;
pub use calibration::{CALIBRATION_PROCEDURE, CalibrationProfile};
pub use device::{HidDevice, HidProvider, ScreenSource};
pub use discover::{DongleCandidate, Match, discover};
pub use gesture::{Plan, Step, SystemGesture};
pub use protocol::{DongleInfo, LinkKind, PROTOCOL_VERSION};
pub use report::{REPORT_MAP, Report};
pub use transport::{Link, MemoryTransport, SerialTransport, SimHandle, Transport};
