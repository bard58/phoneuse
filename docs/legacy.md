# The Python version (phoneuse)

agentphone started as a fork of [phoneuse](https://github.com/lxe/phoneuse) by
Aleksey Smolenchuk (MIT license). The original Python code is in
`legacy/python/`. agentphone does not use it at runtime.

## What phoneuse does

phoneuse lets a Linux PC see and operate an unmodified iPhone.

- Screen: the iPhone sends its screen with AirPlay Screen Mirroring. The UxPlay
  receiver gets the video on the PC.
- Input: the PC acts as a Bluetooth mouse and keyboard. iOS AssistiveTouch
  changes the pointer into touches.
- Agent: an external agent (for example Claude Code) uses it through an MCP
  server or a command line. phoneuse makes no LLM API calls.

phoneuse needs no cable, no developer account, and no Developer Mode. But it
needs Linux with BlueZ, systemd, Hyprland, GTK 4, GStreamer, and root rights for
the Bluetooth daemon.

## Why a rewrite

Windows cannot act as a Bluetooth Classic HID device for an iPhone. Thus the
phoneuse input route does not work on Windows. agentphone uses a different
route: Apple developer services over USB or Wi-Fi (iOS 17.4 and later). It also
adds Android, a built-in agent, and one binary for Windows, macOS, and Linux.

## What agentphone keeps from phoneuse

- The MCP tool design (`look`, `tap`, `tap_on`, `swipe`, and others).
- The settle algorithm (`frames.py`), ported to `ap-screen`.
- The OCR text helpers, ported to `ap-vision`.
- The safety rules: confirm before irreversible actions, and no passcode for
  the agent.
- The HID report map and gesture model, ported to the experimental `ap-hid`
  crate.

## Folder contents

| Folder | Content |
|---|---|
| `legacy/python/phoneuse/` | The Python package |
| `legacy/python/bin/`, `scripts/`, `tools/` | Command-line entry points and helper scripts |
| `legacy/python/skill/` | The Claude Code skill |
| `legacy/python/docs/`, `ARCHITECTURE.md` | The original documentation |
| `legacy/python/touchlab/` | Gesture measurement tools |

To use the Python version, read `legacy/python/README.md`.
