# AgentPhone HID dongle firmware (ESP32-S3, BLE)

> **Status: UNTESTED ON HARDWARE.** This firmware was not compiled or flashed
> yet. The protocol logic matches the tested simulator in
> `crates/ap-hid/src/transport.rs`. The NimBLE API calls follow NimBLE-Arduino 2.x
> and can need small fixes at the first build.

The ESP32-S3 shows itself to the iPhone as a Bluetooth LE HID device, "AgentPhone HID":
a keyboard, a relative mouse, an absolute pointer and a consumer Eject key. The
report map is the same as upstream phoneuse (`src/report_map.h`). The PC sends
reports over USB serial with the AgentPhone protocol v1 (COBS frames, CRC-16,
HELLO handshake, ACK per message). See `crates/ap-hid/src/protocol.rs`.

This folder is not part of the Cargo workspace.

## What you need

- An ESP32-S3 board, for example ESP32-S3-DevKitC-1 (about 10 USD).
- A USB data cable to the PC.
- An iPhone with iOS 13 or later (AssistiveTouch pointer support).

## Flash on Windows

1. Install [VS Code](https://code.visualstudio.com/) and the **PlatformIO IDE** extension.
   Or install the CLI only: `py -3 -m pip install -U platformio`.
2. Connect the board to the PC.
   - Native USB port (marked `USB`): use the environment `esp32s3-native-usb`.
   - UART port (marked `UART` or `COM`): use `esp32s3-uart`. Install the
     [CP210x driver](https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers)
     or the CH340 driver if Device Manager shows an unknown device.
3. Find the port: Device Manager > Ports (COM & LPT), for example `COM5`.
4. Build and flash from this folder:

   ```powershell
   cd firmware
   pio run -e esp32s3-native-usb -t upload --upload-port COM5
   ```

5. If the upload does not start: hold **BOOT**, press and release **RST**, release
   **BOOT**, and run the command again.
6. Optional check: `pio device monitor -p COM5`. The output is binary frames, so
   you only see some text such as `agentphone hid: ready`. Close the monitor
   before you use agentphone: only one program can open the port.
7. Check from agentphone (when ap-cli wires in ap-hid): `ap_hid::discover()` lists the port (VID 303A or the
   UART bridge). The connection does a HELLO handshake and shows the firmware version.

## Pair the iPhone

1. On the iPhone: **Settings > Bluetooth**. Select **AgentPhone HID** under
   Other Devices. Accept the pairing request (Just Works, no PIN).
2. **Settings > Accessibility > Touch > AssistiveTouch**: turn it **on**.
3. In AssistiveTouch, set **Dwell Control** to **off** (a resting pointer must not tap).
4. Optional: **Settings > Accessibility > Pointer Control**: Automatically Hide
   Pointer off, larger pointer, a strong colour (helps when you watch it).
5. Optional: **Settings > Display & Brightness > Auto-Lock > Never** while an agent works.
6. Move the pointer once with agentphone (`HidDevice::tap` at 0.5, 0.5). The
   AssistiveTouch pointer must appear and tap.

To remove the pairing: Settings > Bluetooth > AgentPhone HID > Forget This Device.
To clear the bonds on the board, erase the flash: `pio run -e esp32s3-native-usb -t erase`
and flash again.

## Calibrate (one time per iPhone model)

Drags use a measured table of "report size to points moved". Only the iPhone 17
Pro table (over Bluetooth Classic) is measured. Over BLE, and on other models,
run the calibration routine: `ap_hid::CALIBRATION_PROCEDURE` describes it.

## Known risks

- **Absolute pointer over BLE is not verified.** Upstream measured it over
  Bluetooth Classic HID on iOS 26. iOS can treat the BLE report 3 differently, or
  ignore it. Without absolute positioning the gesture model does not work
  (taps carry the button only in the absolute report).
- **BLE timing.** BLE sends reports in connection events (iOS gives HID devices
  about 11.25 to 15 ms). The model sends one relative report every 8 ms, so two
  reports can arrive in one event. iOS drops motion from reports that come less
  than ~8 ms apart. Drags can be short. Calibrate over BLE, and if needed raise
  the tick to 15 ms.
- **Serial on boards with a UART bridge**: DTR/RTS changes can reset the chip.
  agentphone sets DTR only.

## Not done yet

- USB HID mode (ESP32-S3 TinyUSB or RP2040 into the iPhone through the Lightning
  or USB-C camera adapter, PC on a second UART). The protocol has a link kind
  field (`LINK_USB = 2`) for it.
