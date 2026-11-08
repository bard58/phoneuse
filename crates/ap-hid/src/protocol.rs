//! The serial protocol between the PC and the dongle firmware (version 1).
//!
//! # Framing
//!
//! Each message is `COBS(type, seq, body..., crc16_lo, crc16_hi)` followed by one
//! `0x00` delimiter. CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) covers
//! `type, seq, body`. A decoded frame is at most [`MAX_FRAME`] bytes. Bytes that
//! do not decode (boot log text, noise) are dropped up to the next `0x00`.
//!
//! # Messages
//!
//! | dir | type | name | body |
//! |-----|------|------|------|
//! | PC→ | 0x01 | HELLO | `"APHD"`, protocol version |
//! | PC→ | 0x02 | PING | - |
//! | PC→ | 0x10 | REPORT | report id, payload (see [`crate::report`]) |
//! | PC→ | 0x11 | RELEASE_ALL | - (keys up, buttons up at the last absolute position, eject up) |
//! | →PC | 0x81 | HELLO_ACK | `"APHD"`, protocol version, fw major, minor, patch, link kind, flags, name (UTF-8) |
//! | →PC | 0x82 | PONG | flags |
//! | →PC | 0x90 | ACK | status |
//! | →PC | 0x91 | LOG | UTF-8 text (unsolicited, seq 0) |
//!
//! Every PC message gets exactly one reply with the same `seq`: HELLO→HELLO_ACK,
//! PING→PONG, others→ACK. A device that does not speak version 1 answers HELLO
//! with HELLO_ACK carrying its own version, and ACK `BAD_VERSION` to the rest.
//! Before a successful HELLO the device answers ACK `NOT_READY`.
//!
//! Flags bit 0: a phone is connected to the dongle's HID side.

use ap_core::{Error, Result};

/// Protocol version this crate speaks.
pub const PROTOCOL_VERSION: u8 = 1;
/// Magic in HELLO / HELLO_ACK.
pub const MAGIC: &[u8; 4] = b"APHD";
/// Largest decoded frame (type + seq + body + crc).
pub const MAX_FRAME: usize = 64;

pub mod msg {
    pub const HELLO: u8 = 0x01;
    pub const PING: u8 = 0x02;
    pub const REPORT: u8 = 0x10;
    pub const RELEASE_ALL: u8 = 0x11;
    pub const HELLO_ACK: u8 = 0x81;
    pub const PONG: u8 = 0x82;
    pub const ACK: u8 = 0x90;
    pub const LOG: u8 = 0x91;
}

/// ACK status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Ok = 0,
    /// No phone connected to the dongle (BLE not paired/connected, USB not enumerated).
    NotConnected = 1,
    /// CRC or COBS error.
    BadFrame = 2,
    BadVersion = 3,
    UnknownType = 4,
    BadReport = 5,
    /// HELLO not received yet.
    NotReady = 6,
    /// HID stack refused the report (queue full).
    Busy = 7,
}

impl Status {
    pub fn from_u8(v: u8) -> Option<Status> {
        Some(match v {
            0 => Status::Ok,
            1 => Status::NotConnected,
            2 => Status::BadFrame,
            3 => Status::BadVersion,
            4 => Status::UnknownType,
            5 => Status::BadReport,
            6 => Status::NotReady,
            7 => Status::Busy,
            _ => return None,
        })
    }
}

/// What the dongle presents to the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Ble,
    Usb,
    Unknown(u8),
}

impl LinkKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => LinkKind::Ble,
            2 => LinkKind::Usb,
            n => LinkKind::Unknown(n),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            LinkKind::Ble => 1,
            LinkKind::Usb => 2,
            LinkKind::Unknown(n) => n,
        }
    }
}

/// Answer to HELLO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DongleInfo {
    pub protocol: u8,
    pub firmware: (u8, u8, u8),
    pub link: LinkKind,
    pub phone_connected: bool,
    pub name: String,
}

/// A PC-to-dongle message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostMsg {
    Hello {
        version: u8,
    },
    Ping,
    /// Report id followed by payload.
    Report(Vec<u8>),
    ReleaseAll,
}

/// A dongle-to-PC message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceMsg {
    HelloAck(DongleInfo),
    Pong { phone_connected: bool },
    Ack(u8),
    Log(String),
}

/// A decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub seq: u8,
    pub body: Vec<u8>,
}

impl HostMsg {
    pub fn to_frame(&self, seq: u8) -> Frame {
        let (kind, body) = match self {
            HostMsg::Hello { version } => {
                let mut b = MAGIC.to_vec();
                b.push(*version);
                (msg::HELLO, b)
            }
            HostMsg::Ping => (msg::PING, vec![]),
            HostMsg::Report(r) => (msg::REPORT, r.clone()),
            HostMsg::ReleaseAll => (msg::RELEASE_ALL, vec![]),
        };
        Frame { kind, seq, body }
    }

    pub fn from_frame(f: &Frame) -> Result<HostMsg> {
        Ok(match f.kind {
            msg::HELLO if f.body.len() >= 5 && &f.body[..4] == MAGIC => {
                HostMsg::Hello { version: f.body[4] }
            }
            msg::PING => HostMsg::Ping,
            msg::REPORT if !f.body.is_empty() => HostMsg::Report(f.body.clone()),
            msg::RELEASE_ALL => HostMsg::ReleaseAll,
            k => {
                return Err(Error::protocol(format!(
                    "unexpected host message 0x{k:02x}"
                )));
            }
        })
    }
}

impl DeviceMsg {
    pub fn to_frame(&self, seq: u8) -> Frame {
        let (kind, body) = match self {
            DeviceMsg::HelloAck(i) => {
                let mut b = MAGIC.to_vec();
                b.extend([
                    i.protocol,
                    i.firmware.0,
                    i.firmware.1,
                    i.firmware.2,
                    i.link.to_u8(),
                    u8::from(i.phone_connected),
                ]);
                let room = MAX_FRAME - 4 - b.len();
                b.extend(i.name.as_bytes().iter().take(room));
                (msg::HELLO_ACK, b)
            }
            DeviceMsg::Pong { phone_connected } => (msg::PONG, vec![u8::from(*phone_connected)]),
            DeviceMsg::Ack(s) => (msg::ACK, vec![*s]),
            DeviceMsg::Log(t) => (
                msg::LOG,
                t.as_bytes().iter().take(MAX_FRAME - 4).copied().collect(),
            ),
        };
        Frame { kind, seq, body }
    }

    pub fn from_frame(f: &Frame) -> Result<DeviceMsg> {
        let b = &f.body;
        Ok(match f.kind {
            msg::HELLO_ACK => {
                if b.len() < 10 || &b[..4] != MAGIC {
                    return Err(Error::protocol("HELLO_ACK without APHD magic"));
                }
                DeviceMsg::HelloAck(DongleInfo {
                    protocol: b[4],
                    firmware: (b[5], b[6], b[7]),
                    link: LinkKind::from_u8(b[8]),
                    phone_connected: b[9] & 1 != 0,
                    name: String::from_utf8_lossy(&b[10..]).into_owned(),
                })
            }
            msg::PONG if !b.is_empty() => DeviceMsg::Pong {
                phone_connected: b[0] & 1 != 0,
            },
            msg::ACK if !b.is_empty() => DeviceMsg::Ack(b[0]),
            msg::LOG => DeviceMsg::Log(String::from_utf8_lossy(b).into_owned()),
            k => {
                return Err(Error::protocol(format!(
                    "unexpected device message 0x{k:02x}"
                )));
            }
        })
    }
}

/// CRC-16/CCITT-FALSE.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xffff;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// COBS-encode `data` (no trailing delimiter).
pub fn cobs_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 254 + 2);
    let mut code_idx = 0;
    out.push(0);
    let mut code: u8 = 1;
    for &b in data {
        if b == 0 {
            out[code_idx] = code;
            code_idx = out.len();
            out.push(0);
            code = 1;
        } else {
            out.push(b);
            code += 1;
            if code == 0xff {
                out[code_idx] = code;
                code_idx = out.len();
                out.push(0);
                code = 1;
            }
        }
    }
    out[code_idx] = code;
    out
}

/// COBS-decode one frame (without the delimiter).
pub fn cobs_decode(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let code = data[i];
        if code == 0 {
            return Err(Error::protocol("COBS: zero byte inside frame"));
        }
        i += 1;
        let n = usize::from(code - 1);
        if i + n > data.len() {
            return Err(Error::protocol("COBS: truncated frame"));
        }
        out.extend_from_slice(&data[i..i + n]);
        i += n;
        if code != 0xff && i < data.len() {
            out.push(0);
        }
    }
    Ok(out)
}

/// Encode a frame to wire bytes, including the `0x00` delimiter.
pub fn encode(f: &Frame) -> Vec<u8> {
    let mut raw = Vec::with_capacity(f.body.len() + 4);
    raw.push(f.kind);
    raw.push(f.seq);
    raw.extend_from_slice(&f.body);
    let crc = crc16(&raw);
    raw.extend(crc.to_le_bytes());
    let mut out = cobs_encode(&raw);
    out.push(0);
    out
}

/// Decode wire bytes of one frame (without the delimiter).
pub fn decode(wire: &[u8]) -> Result<Frame> {
    let raw = cobs_decode(wire)?;
    if raw.len() < 4 || raw.len() > MAX_FRAME {
        return Err(Error::protocol(format!(
            "frame length {} out of range",
            raw.len()
        )));
    }
    let (data, crc) = raw.split_at(raw.len() - 2);
    if crc16(data) != u16::from_le_bytes([crc[0], crc[1]]) {
        return Err(Error::protocol("frame CRC mismatch"));
    }
    Ok(Frame {
        kind: data[0],
        seq: data[1],
        body: data[2..].to_vec(),
    })
}

/// Splits a byte stream at `0x00` delimiters.
#[derive(Debug, Default)]
pub struct Deframer {
    buf: Vec<u8>,
}

impl Deframer {
    /// Feed bytes; returns complete wire frames (without delimiters). Over-long
    /// garbage is dropped.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == 0 {
                if !self.buf.is_empty() {
                    out.push(std::mem::take(&mut self.buf));
                }
            } else if self.buf.len() < MAX_FRAME * 2 + 8 {
                self.buf.push(b);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_check_value() {
        assert_eq!(crc16(b"123456789"), 0x29b1);
    }

    #[test]
    fn cobs_round_trip() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![0, 0],
            vec![1, 2, 0, 3],
            (1..=254).collect(),
            (0..=255).collect(),
        ];
        for c in cases {
            let e = cobs_encode(&c);
            assert!(!e.contains(&0), "{c:?}");
            assert_eq!(cobs_decode(&e).unwrap(), c);
        }
        assert_eq!(cobs_encode(&[0x11, 0x00, 0x22]), [0x02, 0x11, 0x02, 0x22]);
    }

    #[test]
    fn frame_wire_bytes() {
        let f = HostMsg::Report(vec![3, 1, 0xff, 0x7f, 0, 0]).to_frame(7);
        let wire = encode(&f);
        assert_eq!(*wire.last().unwrap(), 0);
        assert_eq!(wire.iter().filter(|b| **b == 0).count(), 1);
        let back = decode(&wire[..wire.len() - 1]).unwrap();
        assert_eq!(back, f);
        let mut bad = wire.clone();
        bad[3] ^= 0x40;
        assert!(decode(&bad[..bad.len() - 1]).is_err());
        // HELLO v1, seq 0: 01 00 'A' 'P' 'H' 'D' 01 crc_lo crc_hi
        let h = encode(&HostMsg::Hello { version: 1 }.to_frame(0));
        let raw = cobs_decode(&h[..h.len() - 1]).unwrap();
        assert_eq!(&raw[..7], &[0x01, 0x00, b'A', b'P', b'H', b'D', 0x01]);
    }

    #[test]
    fn messages_round_trip() {
        let info = DongleInfo {
            protocol: 1,
            firmware: (0, 1, 0),
            link: LinkKind::Ble,
            phone_connected: true,
            name: "AgentPhone HID".into(),
        };
        for m in [
            DeviceMsg::HelloAck(info),
            DeviceMsg::Pong {
                phone_connected: false,
            },
            DeviceMsg::Ack(Status::NotConnected as u8),
            DeviceMsg::Log("boot".into()),
        ] {
            let f = decode_wire(&encode(&m.to_frame(9)));
            assert_eq!(DeviceMsg::from_frame(&f).unwrap(), m);
        }
        for m in [
            HostMsg::Hello { version: 1 },
            HostMsg::Ping,
            HostMsg::ReleaseAll,
        ] {
            let f = decode_wire(&encode(&m.to_frame(1)));
            assert_eq!(HostMsg::from_frame(&f).unwrap(), m);
        }
    }

    #[test]
    fn deframer_skips_garbage() {
        let mut d = Deframer::default();
        let mut stream = b"ESP-ROM boot log\r\n".to_vec();
        stream.push(0);
        stream.extend(encode(&HostMsg::Ping.to_frame(3)));
        let frames = d.push(&stream);
        assert_eq!(frames.len(), 2);
        assert!(decode(&frames[0]).is_err());
        assert_eq!(decode(&frames[1]).unwrap().seq, 3);
    }

    fn decode_wire(w: &[u8]) -> Frame {
        decode(&w[..w.len() - 1]).unwrap()
    }
}
