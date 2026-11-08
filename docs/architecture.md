# Architecture

agentphone is a Cargo workspace. The binary is `agentphone` (crate `ap-cli`).
All crates use Rust edition 2024 and deny `unsafe` code.

## Crate map

| Crate | Purpose |
|---|---|
| `ap-core` | Shared types: `Point` (0..1), `Size`, `Frame`, `Button`, `Key`, `Gesture`, the `Device` and `DeviceProvider` traits, `Error`, and `Config`. |
| `ap-ios` | iPhone backend (iOS 17.4 and later) through the `idevice` crate, pinned to `=0.1.68`. |
| `ap-android` | Android backend through a small async client for the local adb server. |
| `ap-screen` | Settle detection, frame strips, downscale and JPEG encoding, coordinate grid. Works on any `Device`. |
| `ap-vision` | Optional OCR (`ocr` feature, `ocrs` engine). Models download at runtime with SHA-256 verification. |
| `ap-llm` | LLM clients: Anthropic Messages API and OpenAI-compatible `/chat/completions`. Prices for cost estimates. |
| `ap-agent` | The agent loop: observe, call the model, act, repeat. Safety gates and take-over. |
| `ap-mcp` | MCP server over stdio and loopback HTTP. 16 tools. |
| `ap-jev` | Optional Jev screen judgments (TypeSafe API). Off by default. |
| `ap-cli` | The `agentphone` command, setup wizard, and `doctor`. |
| `ap-hid` | Experimental. See [Experimental parts](#experimental-parts). |

```
ap-cli ─┬─> ap-agent ──> ap-llm, ap-screen
        ├─> ap-mcp ────> ap-screen, ap-vision
        ├─> ap-ios ────> idevice ──> usbmuxd / Wi-Fi
        ├─> ap-android ─> adb server (TCP 5037)
        └─> ap-jev (feature "jev")
All crates depend on ap-core.
```

## The `Device` trait

Every backend gives a `DeviceProvider` and a `Device`. The front ends
(`ap-cli`, `ap-agent`, `ap-mcp`) use only these traits.

- `DeviceProvider`: `name()`, `discover()` returns a list of `DeviceInfo`,
  `connect(id)` returns a `Box<dyn Device>`.
- `Device`: `info()`, `capabilities()`, `screenshot()`, `tap()`,
  `double_tap()`, `long_press()`, `swipe()`, `scroll()`, `type_text()`,
  `key()`, `button()`, `launch_app()`. `list_apps()`, `ui_tree()`, and
  `home()` have default bodies.
- `Capabilities` tells which actions the backend can do: `tap`, `swipe`,
  `type_text`, `keys`, `buttons`, `launch_app`, `ui_tree`, `screenshot`.
- All points are normalized: `x` and `y` go from 0.0 to 1.0 from the top-left
  corner. Each backend converts them to its own coordinates.

Errors use `ap_core::Error`. `Error::setup(problem, hint)` carries one line that
tells the user what to do. `doctor`, the CLI, and the MCP `status` tool show
this hint.

## Add a new backend

1. Make a new crate, for example `crates/ap-foo`.
2. Implement `ap_core::DeviceProvider` and `ap_core::Device`.
3. Set `DeviceInfo::backend` to a short name, for example `"foo"`.
4. Return the correct `Capabilities`. Return `Error::unsupported(..)` for an
   action that the backend cannot do.
5. Return `Error::setup(..)` with a hint for each problem that the user can fix.
6. Add the provider to `providers()` in `crates/ap-cli/src/devices.rs`. The CLI,
   the agent, and the MCP server then find the new phones.
7. Optional: add setup checks to `doctor` and steps to the wizard.
8. Test the crate with `ap_core::mock` and a fake transport, as `ap-android`
   does.

A new iPhone route (for example WebDriverAgent) does not need a new crate. It
implements the `IosBackend` trait in `crates/ap-ios/src/backend.rs`. The config
key `ios.backend_order` keeps room for more routes.

## iPhone routes

| iOS | Status | Transport | Input | Screenshot |
|---|---|---|---|---|
| 15 | not supported, update iOS | - | - | - |
| 16 | not supported, update iOS | - | - | - |
| 17.0 to 17.3 | not supported, update iOS | - | - | - |
| 17.4 to 17.x | supported (unverified) | CoreDeviceProxy + jktcp + RSD | CoreDevice HID (can be missing) | DVT |
| 18 | supported (unverified) | CoreDeviceProxy + jktcp + RSD | CoreDevice HID (unknown) | DVT |
| 26 | supported (unverified) | CoreDeviceProxy + jktcp + RSD | CoreDevice HID + displayservice gate | DVT |

Design notes for `ap-ios`:

- Each `IosDevice` has its own OS thread with a 32 MiB stack and a
  current-thread tokio runtime. The `idevice` futures are large, and a debug
  build overflowed the 1 MiB Windows main stack.
- `HidBackend` keeps one tunnel and one displayservice media stream for each
  session. The media stream is the gate for HID input. Two tasks read and drop
  the RTP data.
- Touch uses `UniversalHidServiceClient` with coordinates from 0 to 65535. Keys
  and buttons use `IndigoHidClient`.
- `launch_app` uses DVT process control. `list_apps` uses installation_proxy.
- The App Switcher is two Home presses.

### iPhone risks

| Id | Risk |
|---|---|
| R1 | Nothing ran on a device. |
| R2 | The iOS range of CoreDevice HID is not known. iOS 26 is the main target. |
| R3 | On iOS 18 and 26, the personalized DDI can be refused. Then a Cryptex1 DDI is necessary. |
| R4 | Apple TSS needs Internet for the first mount after each restart. |
| R6 | `usbmuxd` is a closed Apple service on Windows. Without it, there is no USB path. |
| R7 | The displayservice request imitates a Mac. A later iOS can refuse it. |
| R8 | Typing uses a US layout. The pasteboard fallback is unverified. |
| R11 | The Wi-Fi path (RemotePairing and mDNS) is unverified. |

The full list is in `crates/ap-ios/README.md`.

## Agent loop

1. The observer takes a screenshot and waits until the screen is still.
2. `ap-screen` downscales the frame and encodes it as JPEG.
3. `ap-llm` sends the frame and the tool list to the model.
4. `ap-agent` checks the tool call against the safety rules, then calls the
   `Device`.
5. `ap-screen` makes a frame strip of the screen after the action.
6. The loop repeats until the model calls `done`, the step limit is reached, or
   the host cancels the run.

The `AgentHost` trait connects the loop to a front end. It shows step events,
asks for confirmations, and asks questions.

## Experimental parts

These parts are in the repository but are not part of the product. They are not
in the CLI and are not tested on hardware.

- `crates/ap-hid` and `firmware/`: an ESP32-S3 dongle that acts as a Bluetooth
  or USB HID keyboard and pointer. iOS AssistiveTouch changes the pointer into
  touches. This route could support iPhones without Developer Mode. The
  firmware was not compiled, and iOS can refuse the absolute pointer over BLE.
- `ios.backend_order` defaults to `["hid"]`. Other names (for example `wda`)
  are skipped with a warning. The `ios.wda_*` config keys are placeholders. No
  WebDriverAgent backend exists.
