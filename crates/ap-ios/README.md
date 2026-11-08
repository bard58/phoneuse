# ap-ios

The iOS backend of agentphone. It drives an iPhone from Windows, macOS, or Linux. It does not need a Mac. It uses the [idevice](https://crates.io/crates/idevice) crate, pinned to `=0.1.68`.

**Status: compile-tested only.** No iPhone was attached during development. Every device path is unverified. The unit tests cover the pure logic. One runtime check passed: the "usbmuxd missing" error on Windows.

## Routes per iOS version

| iOS | Status | Transport | Input | Screenshot |
|---|---|---|---|---|
| 15 | not supported, update iOS | - | - | - |
| 16 | not supported, update iOS | - | - | - |
| 17.0 - 17.3 | not supported, update iOS | - | - | - |
| 17.4 - 17.x | supported (unverified) | CoreDeviceProxy + jktcp + RSD | CoreDevice HID (expected to be missing, see R2) | DVT |
| 18 | supported (unverified) | CoreDeviceProxy + jktcp + RSD | CoreDevice HID (unknown, see R2) | DVT |
| 26 | supported (unverified) | CoreDeviceProxy + jktcp + RSD | CoreDevice HID + displayservice gate | DVT |

For iOS below 17.4, `discover()` lists the device with backend `"unsupported"`. `connect()` returns `Error::Setup` with this hint: "iOS 17.4 or later is required. Update the iPhone in Settings > General > Software Update."

Run `setup::check` on the real device. If the `hid_services` check fails after the DDI mount, the CoreDevice HID route does not work on that iOS version.

## What the user must do once

1. Install the Apple Devices app from the Microsoft Store (or iTunes). It gives the Apple Mobile Device Service (usbmuxd, `127.0.0.1:27015`).
2. Connect the iPhone with a cable. Tap Trust (`setup::pair`).
3. Turn on Developer Mode (`setup::enable_developer_mode_prompt`). The iPhone restarts. Confirm after the restart.
4. Mount the personalized DDI (`setup::mount_ddi`). agentphone downloads it once from the doronz88/DeveloperDiskImage mirror into the cache folder. The DDI unmounts at each restart, so mount it again after a restart. The first mount after a restart needs Internet (Apple TSS).
5. Optional: save the Wi-Fi pairing (`setup::save_wifi_pairing`).

## Public API

- `IosProvider::new(IosConfig)`: implements `ap_core::DeviceProvider`.
  - `discover()`: usbmuxd devices, then saved Wi-Fi pairings found by mDNS (`_apple-mobdev2._tcp`, `_remotepairing._tcp`) or by the last IP.
  - `connect(id)`: an `IosDevice` as `Box<dyn Device>`. `id` is the UDID or a unique UDID prefix. `connect_ios(id)` gives the concrete type.
- `IosDevice`: implements `ap_core::Device`. It also has `pinch()` and `ios_version()`.
- `setup`: `check(cfg, id) -> Vec<CheckResult>`, and the actions `pair`, `reveal_developer_mode`, `enable_developer_mode_prompt`, `download_ddi`, `mount_ddi`, `save_wifi_pairing`.
- `block_on(fut)`, `runtime_builder()`: big-stack runtime helpers.
- `models`: the iPhone table (`ProductType` to name, points, pixels, newest iOS).
- `keymap`, `coords`, `version`, `store`, `mdns`: helpers. They are public so that the CLI and tests can use them.

## Design

- Each `IosDevice` has its own OS thread. The thread has a 32 MiB stack and a current-thread tokio runtime. The backend lives on that thread. The `Device` methods send jobs to it through a channel. Thus the caller does not need a big stack, and the idevice futures do not need to be `Send`.
- `setup` and `discover` also run on a temporary big-stack thread.
- `HidBackend` keeps one tunnel and one displayservice media stream (the HID auth gate) per session. Two background tasks read and drop the RTP datagrams, so memory does not grow. A transport error closes the session, and the next call opens a new one. Input is not replayed: an input call runs once more only when the error came before its first event. After that, it returns an "input may be partly delivered" error, which is not a transport error. Screenshot, pasteboard and launch get one full retry.
- On an error or a cancel in the middle of a gesture or a key stroke, the backend sends the touch release or key up as best effort.
- Time limits: each attempt has a budget (setup + expected work + slack). The worker thread also stops a job at a backstop deadline and drops the session. Thus one stalled read does not block the device for good. An expired limit is `Error::Timeout`.
- Cancel: `IosDevice::cancel_handle()` gives a `CancelHandle` (`Send + Sync`). `cancel()` stops the calls submitted before it: a queued call does not start, and a running text, drag, long press, pinch or button sequence stops at the next stroke or point. The call returns the error from `ap_ios::cancelled()` (test it with `ap_ios::is_cancelled`).
- `foreground_app` fails closed: it returns the app of the last `launch_app` only until the next input (tap, swipe, scroll, text, key, button, pinch). iOS has no foreground-app query on this route, and any input can open another app.
- Touch uses `UniversalHidServiceClient` with normalized `u16` coordinates (0 to 65535). Keys and buttons use `IndigoHidClient`.
- `type_text` sends US-layout key strokes. If the text has a character without a US key (accents, non-Latin scripts, emoji), it puts the whole text on the pasteboard and sends Cmd+V.
- The App Switcher is a double Home press.
- `launch_app` uses DVT process control. `list_apps` uses installation_proxy (user apps only).
- A new backend (for example WebDriverAgent) plugs in through the `IosBackend` trait. See `src/backend.rs`.

## Windows stack size

The idevice futures are large. A debug build overflowed the 1 MiB Windows main stack in the spike. ap-ios protects itself (see Design). If a binary runs other large futures, use this:

```rust
fn main() -> std::process::ExitCode {
    ap_ios::block_on(async { /* ... */ std::process::ExitCode::SUCCESS })
}
```

`block_on` runs a multi-thread runtime with 8 MiB worker stacks from a 32 MiB thread, and boxes the future.

## Files

The pairing folder is `ios.pairing_dir`, or `%APPDATA%\agentphone\data\ios-pairing` on Windows. It holds `<udid>.plist` (lockdown pairing), `<udid>.rppairing` (RemotePairing), and `<udid>.json` (name, model, last IP). These files are secrets: they give control of the iPhone. On Unix the folder is mode 700 and the files are mode 600 from creation. On Windows the files get the ACL of the folder. The default folder is private to the user. A custom `pairing_dir` must be a folder that only the user can read.

The DDI cache is `<cache dir>/ddi/Xcode_iOS_DDI_Personalized/`. Set `AGENTPHONE_DDI_MIRROR` to use another mirror.

## Risks

- R1: Nothing ran on a device. The API use follows the idevice v0.1.68 tools and the spike. The HID gate and the coordinate space are only documented in the idevice source.
- R2: The iOS range of CoreDevice HID is not known. The idevice HID code imitates Xcode 26 Device Hub, so iOS 26 is the tested target. On 17.4 to 18 the HID services can be missing.
- R3: The DDI type on iOS 18 and 26. If `mount_personalized` is refused, ap-ios needs the idevice `cryptexd` feature (Cryptex1 DDI from Xcode 26). The old personalized image may not contain `dtuhidd` or displayservice.
- R4: Apple TSS needs Internet for the first mount after each restart.
- R5: Windows stack overflow. ap-ios uses big-stack threads (see above).
- R6: usbmuxd is a closed Apple service. Without the Apple Devices app there is no USB path. Wi-Fi needs a pairing made over USB first.
- R7: The displayservice offer imitates a Mac (`Mac17,7`, `25F71`). A later iOS can refuse it.
- R8: Typing uses a US layout. Another hardware-keyboard layout on the iPhone can change the symbols. The pasteboard fallback is unverified.
- R9: agentphone cannot turn on Developer Mode when a passcode is set. The user must use Settings.
- R10: The iPhone 17 family identifiers (`iPhone18,x`) in the model table come from public lists. They are not verified.
- R11: Wi-Fi: CoreDeviceProxy over the lockdown TCP path needs `EnableWifiConnections`. `save_wifi_pairing` sets it. The RemotePairing path and the mDNS matching are unverified.
