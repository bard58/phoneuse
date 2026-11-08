// AgentPhone serial protocol v1 (device side). Must match crates/ap-hid/src/protocol.rs.
//
// Wire: COBS(type, seq, body..., crc16_lo, crc16_hi) 0x00
// CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF) over type, seq, body.
#pragma once

#include <stddef.h>
#include <stdint.h>
#include <string.h>

namespace ap {

static const uint8_t PROTOCOL_VERSION = 1;
static const uint8_t MAGIC[4] = {'A', 'P', 'H', 'D'};
static const size_t MAX_FRAME = 64;  // decoded: type + seq + body + crc

enum Msg : uint8_t {
  HELLO = 0x01,
  PING = 0x02,
  REPORT = 0x10,
  RELEASE_ALL = 0x11,
  HELLO_ACK = 0x81,
  PONG = 0x82,
  ACK = 0x90,
  LOG = 0x91,
};

enum Status : uint8_t {
  OK = 0,
  NOT_CONNECTED = 1,
  BAD_FRAME = 2,
  BAD_VERSION = 3,
  UNKNOWN_TYPE = 4,
  BAD_REPORT = 5,
  NOT_READY = 6,
  BUSY = 7,
};

enum LinkKind : uint8_t { LINK_BLE = 1, LINK_USB = 2 };

inline uint16_t crc16(const uint8_t* d, size_t n) {
  uint16_t crc = 0xFFFF;
  for (size_t i = 0; i < n; i++) {
    crc ^= (uint16_t)d[i] << 8;
    for (int b = 0; b < 8; b++) crc = (crc & 0x8000) ? (crc << 1) ^ 0x1021 : (crc << 1);
  }
  return crc;
}

// Encode n bytes into out (needs n + n/254 + 1 bytes). Returns the encoded length
// (without the 0x00 delimiter).
inline size_t cobs_encode(const uint8_t* in, size_t n, uint8_t* out) {
  size_t code_idx = 0, o = 1;
  uint8_t code = 1;
  for (size_t i = 0; i < n; i++) {
    if (in[i] == 0) {
      out[code_idx] = code;
      code_idx = o++;
      code = 1;
    } else {
      out[o++] = in[i];
      if (++code == 0xFF) {
        out[code_idx] = code;
        code_idx = o++;
        code = 1;
      }
    }
  }
  out[code_idx] = code;
  return o;
}

// Decode n bytes (no delimiter) into out (max cap). Returns the length, or -1.
inline int cobs_decode(const uint8_t* in, size_t n, uint8_t* out, size_t cap) {
  size_t i = 0, o = 0;
  while (i < n) {
    uint8_t code = in[i++];
    if (code == 0) return -1;
    size_t len = code - 1;
    if (i + len > n || o + len > cap) return -1;
    memcpy(out + o, in + i, len);
    o += len;
    i += len;
    if (code != 0xFF && i < n) {
      if (o >= cap) return -1;
      out[o++] = 0;
    }
  }
  return (int)o;
}

}  // namespace ap
