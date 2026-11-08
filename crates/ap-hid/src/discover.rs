//! Find serial ports that may be an AgentPhone dongle.
//!
//! A port matches strongly when its USB product string contains
//! [`PRODUCT_TAG`] (the firmware sets it where the USB stack allows). It matches
//! weakly when its USB VID/PID belongs to a supported board. The handshake in
//! [`crate::HidDevice::open`] is the final check.

use ap_core::{Error, Result};
use serde::Serialize;
use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};

/// Text in the USB product string of the firmware.
pub const PRODUCT_TAG: &str = "AgentPhone";

/// USB VID/PID pairs of supported boards.
pub const KNOWN_USB_IDS: &[(u16, u16, &str)] = &[
    (0x303a, 0x1001, "ESP32-S3 USB-Serial/JTAG (native USB port)"),
    (
        0x303a,
        0x4001,
        "ESP32-S3 TinyUSB CDC (Arduino, USB-OTG mode)",
    ),
    (0x303a, 0x0002, "ESP32-S2/S3 TinyUSB CDC"),
    (0x2e8a, 0x000a, "Raspberry Pi RP2040 (Pico SDK CDC)"),
    (0x2e8a, 0x00c0, "Raspberry Pi RP2040 (Arduino)"),
    (
        0x10c4,
        0xea60,
        "CP210x UART bridge (ESP32-S3 DevKitC UART port)",
    ),
    (0x1a86, 0x55d4, "CH9102 UART bridge"),
    (0x1a86, 0x7523, "CH340 UART bridge"),
];

/// How a port matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    /// The VID/PID is a supported board; the firmware is not confirmed.
    UsbId,
    /// The product string names the firmware.
    Product,
}

/// A candidate port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DongleCandidate {
    pub port: String,
    pub vid: u16,
    pub pid: u16,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub serial_number: Option<String>,
    pub matched: Match,
    /// Board description from [`KNOWN_USB_IDS`], if any.
    pub board: Option<&'static str>,
}

/// Classify one USB port.
pub fn classify(port: &str, usb: &UsbPortInfo) -> Option<DongleCandidate> {
    let board = KNOWN_USB_IDS
        .iter()
        .find(|(v, p, _)| *v == usb.vid && *p == usb.pid)
        .map(|(_, _, d)| *d);
    let named = [&usb.product, &usb.manufacturer]
        .iter()
        .any(|s| s.as_deref().is_some_and(|s| s.contains(PRODUCT_TAG)));
    let matched = if named {
        Match::Product
    } else if board.is_some() {
        Match::UsbId
    } else {
        return None;
    };
    Some(DongleCandidate {
        port: port.to_string(),
        vid: usb.vid,
        pid: usb.pid,
        product: usb.product.clone(),
        manufacturer: usb.manufacturer.clone(),
        serial_number: usb.serial_number.clone(),
        matched,
        board,
    })
}

/// Filter a port list; strong matches first.
pub fn filter_ports(ports: &[SerialPortInfo]) -> Vec<DongleCandidate> {
    let mut out: Vec<DongleCandidate> = ports
        .iter()
        .filter_map(|p| match &p.port_type {
            SerialPortType::UsbPort(u) => classify(&p.port_name, u),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| b.matched.cmp(&a.matched).then_with(|| a.port.cmp(&b.port)));
    out
}

/// List serial ports that may be an AgentPhone dongle. No port is opened.
pub fn discover() -> Result<Vec<DongleCandidate>> {
    let ports = serialport::available_ports().map_err(|e| {
        Error::setup(
            format!("cannot list serial ports: {e}"),
            "On Windows, check Device Manager > Ports (COM & LPT). Install the CP210x or CH340 \
             driver if the board uses a UART bridge.",
        )
    })?;
    Ok(filter_ports(&ports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(vid: u16, pid: u16, product: Option<&str>) -> UsbPortInfo {
        UsbPortInfo {
            vid,
            pid,
            serial_number: None,
            manufacturer: None,
            product: product.map(String::from),
        }
    }

    #[test]
    fn matches() {
        let ports = vec![
            SerialPortInfo {
                port_name: "COM3".into(),
                port_type: SerialPortType::UsbPort(usb(0x303a, 0x1001, None)),
            },
            SerialPortInfo {
                port_name: "COM9".into(),
                port_type: SerialPortType::UsbPort(usb(0x1234, 0x5678, Some("AgentPhone HID"))),
            },
            SerialPortInfo {
                port_name: "COM4".into(),
                port_type: SerialPortType::UsbPort(usb(0x046d, 0xc52b, Some("Logitech"))),
            },
            SerialPortInfo {
                port_name: "COM1".into(),
                port_type: SerialPortType::Unknown,
            },
        ];
        let c = filter_ports(&ports);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].port.as_str(), c[0].matched), ("COM9", Match::Product));
        assert_eq!((c[1].port.as_str(), c[1].matched), ("COM3", Match::UsbId));
    }
}
