// AgentPhone HID dongle: ESP32-S3 as a BLE HID combo (keyboard + relative mouse +
// absolute pointer + consumer Eject), driven by the PC over USB serial.
//
// UNTESTED ON HARDWARE. The protocol logic mirrors the simulator in
// crates/ap-hid/src/transport.rs (sim_reply), which is tested.

#include <Arduino.h>
#include <NimBLEDevice.h>
#include <NimBLEHIDDevice.h>

#include "protocol.h"
#include "report_map.h"

#ifndef AGENTPHONE_FW_MAJOR
#define AGENTPHONE_FW_MAJOR 0
#define AGENTPHONE_FW_MINOR 1
#define AGENTPHONE_FW_PATCH 0
#endif

static const char* DEVICE_NAME = "AgentPhone HID";
// USB-IF test VID/PID style values for the BLE PnP ID. Not a USB device.
static const uint16_t PNP_VID = 0x303A;
static const uint16_t PNP_PID = 0x8A50;

static NimBLEServer* server = nullptr;
static NimBLEHIDDevice* hid = nullptr;
static NimBLECharacteristic* input[5] = {nullptr};  // index = report id

static volatile bool phone_connected = false;
// BLE callbacks run on the NimBLE host task. They only set these; loop() writes
// the LOG frames, so two tasks never write Serial at the same time.
static volatile bool ev_connected = false, ev_disconnected = false, ev_paired = false;
static volatile bool ev_pair_failed = false;
static volatile int ev_reason = 0;
static bool ready = false;  // HELLO received with our version
static uint8_t last_abs[5] = {0};
static bool have_abs = false;

// ---------- serial framing ----------

static uint8_t rx_buf[ap::MAX_FRAME * 2 + 8];
static size_t rx_len = 0;
static bool rx_overflow = false;

static void send_frame(uint8_t type, uint8_t seq, const uint8_t* body, size_t n) {
  uint8_t raw[ap::MAX_FRAME];
  if (n > ap::MAX_FRAME - 4) n = ap::MAX_FRAME - 4;
  raw[0] = type;
  raw[1] = seq;
  memcpy(raw + 2, body, n);
  uint16_t crc = ap::crc16(raw, n + 2);
  raw[n + 2] = crc & 0xFF;
  raw[n + 3] = crc >> 8;
  uint8_t enc[ap::MAX_FRAME + 4];
  size_t m = ap::cobs_encode(raw, n + 4, enc);
  enc[m++] = 0;
  Serial.write(enc, m);
}

static void send_ack(uint8_t seq, uint8_t status) { send_frame(ap::ACK, seq, &status, 1); }

static void send_log(const char* text) {
  send_frame(ap::LOG, 0, (const uint8_t*)text, strlen(text));
}

// ---------- HID ----------

static bool notify(uint8_t id, const uint8_t* payload, size_t n) {
  if (id < 1 || id > 4 || !input[id]) return false;
  input[id]->setValue(payload, n);
  return input[id]->notify();
}

static void release_all() {
  static const uint8_t keys_up[8] = {0};
  static const uint8_t mouse_up[4] = {0};
  static const uint8_t eject_up[1] = {0};
  notify(1, keys_up, sizeof keys_up);
  notify(2, mouse_up, sizeof mouse_up);
  notify(4, eject_up, sizeof eject_up);
  if (have_abs) {
    // Lift where the pointer is: an absolute report at (0, 0) would move it.
    uint8_t up[5];
    memcpy(up, last_abs, 5);
    up[0] = 0;
    notify(3, up, 5);
  }
}

class ServerCallbacks : public NimBLEServerCallbacks {
  void onConnect(NimBLEServer* s, NimBLEConnInfo& info) override {
    phone_connected = true;
    // Ask for a short connection interval (7.5..15 ms). iOS decides; HID
    // accessories usually get 15 ms (or 11.25 ms).
    s->updateConnParams(info.getConnHandle(), 6, 12, 0, 200);
    ev_connected = true;
  }

  void onDisconnect(NimBLEServer* s, NimBLEConnInfo& info, int reason) override {
    phone_connected = false;
    have_abs = false;
    ev_reason = reason;
    ev_disconnected = true;
    NimBLEDevice::startAdvertising();
  }

  void onAuthenticationComplete(NimBLEConnInfo& info) override {
    if (info.isEncrypted()) {
      ev_paired = true;
    } else {
      ev_pair_failed = true;
    }
  }
};

static void ble_setup() {
  NimBLEDevice::init(DEVICE_NAME);
  NimBLEDevice::setPower(ESP_PWR_LVL_P9);
  // Just Works pairing with bonding and secure connections (no PIN on the PC side).
  NimBLEDevice::setSecurityAuth(true, false, true);
  NimBLEDevice::setSecurityIOCap(BLE_HS_IO_NO_INPUT_OUTPUT);

  server = NimBLEDevice::createServer();
  server->setCallbacks(new ServerCallbacks());

  hid = new NimBLEHIDDevice(server);
  hid->setManufacturer("AgentPhone");
  hid->setPnp(0x02, PNP_VID, PNP_PID, 0x0100);
  hid->setHidInfo(0x00, 0x01);  // country 0, flags: remote wake
  hid->setReportMap((uint8_t*)REPORT_MAP, sizeof(REPORT_MAP));
  for (uint8_t id = 1; id <= 4; id++) input[id] = hid->getInputReport(id);
  hid->setBatteryLevel(100);
  hid->startServices();

  NimBLEAdvertising* adv = NimBLEDevice::getAdvertising();
  adv->setAppearance(0x03C0);  // generic HID
  adv->addServiceUUID(hid->getHidService()->getUUID());
  adv->setName(DEVICE_NAME);
  adv->enableScanResponse(true);
  adv->start();
}

// ---------- dispatch (same logic as sim_reply in transport.rs) ----------

static void handle_frame(const uint8_t* wire, size_t n) {
  uint8_t raw[ap::MAX_FRAME];
  int len = ap::cobs_decode(wire, n, raw, sizeof raw);
  if (len < 4) {
    send_ack(0, ap::BAD_FRAME);
    return;
  }
  uint16_t crc = raw[len - 2] | (raw[len - 1] << 8);
  if (ap::crc16(raw, len - 2) != crc) {
    send_ack(0, ap::BAD_FRAME);
    return;
  }
  uint8_t type = raw[0], seq = raw[1];
  const uint8_t* body = raw + 2;
  size_t body_len = len - 4;

  switch (type) {
    case ap::HELLO: {
      if (body_len < 5 || memcmp(body, ap::MAGIC, 4) != 0) {
        send_ack(seq, ap::BAD_REPORT);
        return;
      }
      ready = body[4] == ap::PROTOCOL_VERSION;
      uint8_t out[ap::MAX_FRAME - 4];
      memcpy(out, ap::MAGIC, 4);
      out[4] = ap::PROTOCOL_VERSION;
      out[5] = AGENTPHONE_FW_MAJOR;
      out[6] = AGENTPHONE_FW_MINOR;
      out[7] = AGENTPHONE_FW_PATCH;
      out[8] = ap::LINK_BLE;
      out[9] = phone_connected ? 1 : 0;
      size_t name_len = strlen(DEVICE_NAME);
      if (name_len > sizeof out - 10) name_len = sizeof out - 10;
      memcpy(out + 10, DEVICE_NAME, name_len);
      send_frame(ap::HELLO_ACK, seq, out, 10 + name_len);
      return;
    }
    case ap::PING:
    case ap::REPORT:
    case ap::RELEASE_ALL:
      break;
    default:
      send_ack(seq, ap::UNKNOWN_TYPE);
      return;
  }
  if (!ready) {
    send_ack(seq, ap::NOT_READY);
    return;
  }
  if (type == ap::PING) {
    uint8_t flags = phone_connected ? 1 : 0;
    send_frame(ap::PONG, seq, &flags, 1);
    return;
  }
  if (type == ap::REPORT) {
    if (body_len < 1 || report_len(body[0]) == 0 || (size_t)report_len(body[0]) != body_len - 1) {
      send_ack(seq, ap::BAD_REPORT);
      return;
    }
    if (!phone_connected) {
      send_ack(seq, ap::NOT_CONNECTED);
      return;
    }
    if (body[0] == 3) {
      memcpy(last_abs, body + 1, 5);
      have_abs = true;
    }
    // notify() returns false when the phone has not subscribed yet or the
    // stack's queue is full.
    send_ack(seq, notify(body[0], body + 1, body_len - 1) ? ap::OK : ap::BUSY);
    return;
  }
  // RELEASE_ALL
  if (!phone_connected) {
    send_ack(seq, ap::NOT_CONNECTED);
    return;
  }
  release_all();
  send_ack(seq, ap::OK);
}

static void serial_poll() {
  while (Serial.available() > 0) {
    int c = Serial.read();
    if (c < 0) break;
    if (c == 0) {
      if (rx_len > 0 && !rx_overflow) handle_frame(rx_buf, rx_len);
      rx_len = 0;
      rx_overflow = false;
    } else if (rx_len < sizeof rx_buf) {
      rx_buf[rx_len++] = (uint8_t)c;
    } else {
      rx_overflow = true;  // drop until the next delimiter
    }
  }
}

void setup() {
  Serial.begin(115200);
  ble_setup();
  send_log("agentphone hid: ready");
}

static void flush_events() {
  if (ev_connected) {
    ev_connected = false;
    send_log("ble: connected");
  }
  if (ev_paired) {
    ev_paired = false;
    send_log("ble: paired (encrypted)");
  }
  if (ev_pair_failed) {
    ev_pair_failed = false;
    send_log("ble: pairing failed");
  }
  if (ev_disconnected) {
    ev_disconnected = false;
    char msg[40];
    snprintf(msg, sizeof msg, "ble: disconnected (%d)", ev_reason);
    send_log(msg);
  }
}

void loop() {
  serial_poll();
  flush_events();
  delay(0);  // yield; the NimBLE host runs in its own task
}
