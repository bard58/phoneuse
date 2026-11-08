# agentphone

agentphone lets an AI agent see and operate your iPhone or Android phone from a
Windows, macOS, or Linux computer. It is one Rust binary, `agentphone`. You can
give it a task in plain language (`agentphone run "turn on Wi-Fi"`), or connect
it to Claude Code, Claude Desktop, or Cursor as an MCP server. An iPhone needs
no Mac, no Xcode, and no app on the phone. agentphone is a cross-platform
rewrite of [phoneuse](https://github.com/lxe/phoneuse), which runs on Linux only.

## Status

agentphone is early software. Read this before you use it.

- The code builds and the unit tests pass on Windows.
- Nobody tested agentphone on a real phone yet. Every device path (iPhone and
  Android) is unverified.
- iPhone touch input uses the CoreDevice HID services. Public reports show
  that these services accept input only on iOS 27 release builds and later
  (early iOS 27 betas list no HID service). The iPhone refuses the display
  stream with CoreDevice error 9021. This is reported on iOS 18 and 26, and
  expected on 17.x. Thus touch input does not work on iOS 17.4 to 26.x.
  agentphone then works in "screen only" mode: screenshots, buttons, and keys
  can work, and a touch call stops with "Touch input needs iOS 27 or later".
  Back is an edge swipe, so Back needs touch input too. `agentphone run`
  stops before the first AI model call. Models newer than `iPhone18,5` can
  need a Cryptex1 developer disk image, which agentphone does not support
  yet. `agentphone doctor` shows a warning on the "Input services (touch,
  buttons, keys)" check below iOS 27.
- Report problems in the issue tracker of
  [bard58/phoneuse](https://github.com/bard58/phoneuse/issues).

## Install on Windows

You do not need a Rust toolchain. In PowerShell, run this command:

```powershell
irm https://raw.githubusercontent.com/bard58/phoneuse/rust-rewrite/scripts/install.ps1 | iex
```

The script downloads `agentphone.exe` from the latest release. It puts the
file in `%LOCALAPPDATA%\agentphone\bin` and adds that folder to your user
`PATH`. It does not need admin rights. Then it checks for the Apple Devices
app and runs `agentphone doctor`.

Manual alternative:

1. Download the Windows `.zip` file from the
   [releases page](https://github.com/bard58/phoneuse/releases), or build
   with `cargo` (see step 1 below).
2. Put `agentphone.exe` in a folder that is on your `PATH`.
3. Run `agentphone doctor`.

## Quick start (Windows)

1. Get the binary. Download `agentphone.exe` from the
   [releases page](https://github.com/bard58/phoneuse/releases)
   (no release is published yet), or build it with Rust 1.88 or later:

   ```powershell
   cargo install --git https://github.com/bard58/phoneuse --branch rust-rewrite ap-cli
   ```

   The `ocr` feature needs Rust 1.94 or later.

2. Open a terminal and run `agentphone`. On the first run, the setup wizard
   starts.
3. Do the wizard steps. It sets up the phone, the AI model, and Claude Code.
   For an iPhone, see [docs/iphone.md](docs/iphone.md). For Android, see
   [docs/android.md](docs/android.md).
4. Give the agent a task:

   ```powershell
   agentphone run "open Settings and turn on Wi-Fi"
   ```

5. Optional: add agentphone to Claude Code as an MCP server:

   ```powershell
   claude mcp add --scope user agentphone -- agentphone mcp
   ```

Run `agentphone doctor` when something does not work.

## Safety in short

- The agent asks you before it sends, pays, buys, or deletes. agentphone also
  asks you before Enter, before text with a newline, and before a tap right
  after typing in a messaging, payment, or shopping app.
- agentphone refuses digit runs that look like a passcode, unless you approve
  that exact call.
- Push Ctrl+C to stop the agent. This is the hard stop. The other checks lower
  the risk, but they depend partly on the AI model and on what is on the
  screen.

See [docs/agent.md](docs/agent.md#safety-rules) for the full rules.

## Supported devices

| Phone | Requirement | Connection | Status |
|---|---|---|---|
| iPhone 11 and newer | iOS 17.4 or later, Developer Mode, one USB pairing. Touch input needs an iOS 27 release build or later. Models newer than `iPhone18,5` can need a Cryptex1 disk image, which agentphone does not support yet. | USB, then Wi-Fi | unverified on a device |
| iPhone 13 | iOS 17.4 to 27. Touch input needs iOS 27. Below 27: screen only. | USB, then Wi-Fi | target model, unverified on a device |
| iPhone XS, XS Max, XR | iOS 17.4 to 18 (the last iOS for these models), Developer Mode, one USB pairing | USB, then Wi-Fi | agentphone connects in screen only mode. Touch input does not work, because these models cannot get iOS 27. Screenshots, buttons, and keys can work. Unverified on a device. |
| iPhone with iOS 17.3 or earlier | not supported. Update iOS. | - | - |
| Android phone | USB debugging | USB, or Wi-Fi (Android 11 or later, `adb pair`) | unverified on a device |

| Computer | iPhone needs | Android needs |
|---|---|---|
| Windows 10 or 11 | Apple Devices app (Microsoft Store) or iTunes | Google platform-tools (`adb`) |
| macOS | nothing more | Google platform-tools (`adb`) |
| Linux | `usbmuxd` | Google platform-tools (`adb`) |

agentphone needs no administrator rights.

## Commands

| Command | What it does |
|---|---|
| `agentphone setup [ios\|android\|llm\|mcp\|ocr]` | Step-by-step setup. You can run it again. |
| `agentphone doctor` | Examines the setup and prints one fix for each problem. |
| `agentphone selftest [--out DIR] [--yes]` | First-run test on the phone: setup, connection, screenshot, then Home and one swipe after you answer y. The swipe is sent only on the home screen. On iOS below 27 the input test is skipped. Writes `selftest-report.json` for the developer. |
| `agentphone devices`, `use <id>`, `info` | Lists phones, sets the default phone, shows details. |
| `agentphone screenshot [-o FILE] [--grid]` | Saves a PNG of the screen. |
| `agentphone tap X Y`, `long-press X Y`, `swipe X1 Y1 X2 Y2` | Touch input. X and Y are fractions from 0 to 1. Add `--px` for pixels of the saved screenshot. |
| `agentphone scroll up\|down\|left\|right [--amount F] [--at X Y]` | Scrolls the content. The amount and `--at` are fractions from 0 to 1. Scroll has no `--px` flag. |
| `agentphone tap-on "TEXT"` | Finds text on the screen and taps it. Needs OCR. |
| `agentphone type "TEXT"`, `key KEY`, `button NAME` | Keyboard and hardware buttons. |
| `agentphone open APP`, `apps`, `ui-tree` | Opens an app, lists apps, prints the UI tree. |
| `agentphone run "TASK"` | The AI agent does the task. |
| `agentphone mcp [--http [ADDR]]` | Starts the MCP server. |
| `agentphone config show\|path\|set KEY VALUE\|edit` | Shows or changes the configuration. |
| `agentphone completions SHELL` | Prints a shell completion script. |

## Documentation

- [docs/iphone.md](docs/iphone.md): iPhone setup on Windows, macOS, and Linux
- [docs/android.md](docs/android.md): Android setup
- [docs/agent.md](docs/agent.md): `agentphone run`, AI providers, cost, safety
- [docs/mcp.md](docs/mcp.md): Claude Code, Claude Desktop, Cursor, and HTTP mode
- [docs/architecture.md](docs/architecture.md): crates, the `Device` trait, new backends
- [docs/troubleshooting.md](docs/troubleshooting.md) and [docs/faq.md](docs/faq.md)
- [docs/legacy.md](docs/legacy.md): the original Python version

## Credits and license

agentphone is a Rust rewrite of [phoneuse](https://github.com/lxe/phoneuse) by
Aleksey Smolenchuk. phoneuse uses the MIT license. Its Python code is in
`legacy/python/`. agentphone also uses the MIT license. See [LICENSE](LICENSE).

The iPhone backend uses the [idevice](https://crates.io/crates/idevice) crate.
