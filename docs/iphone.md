# iPhone setup

This guide sets up an iPhone for agentphone. The steps are the same for all
supported iPhone models. The examples use an iPhone 13.

agentphone does not need a Mac, Xcode, an app on the iPhone, or administrator
rights. It needs a USB cable one time. After the setup, it can also use Wi-Fi.

> NOTE: Nobody tested agentphone on a real iPhone yet. If a step fails, run
> `agentphone doctor` and see [Troubleshooting](#troubleshooting).

## Requirements

- An iPhone with iOS 17.4 or later. iPhone XS, XR, and newer models can run
  iOS 17.4. iOS 17.3 and earlier are not supported.
- A USB data cable (the cable in the box is correct).
- The passcode of the iPhone. You type it on the iPhone, not in agentphone.
- An Internet connection for the first setup and after each restart of the
  iPhone.

To see your iOS version, open **Settings > General > About > iOS Version**. To
update, open **Settings > General > Software Update**.

## 1. Install the USB service on the computer

agentphone talks to the iPhone through the Apple Mobile Device service
(`usbmuxd`).

### Windows

1. Install the free **Apple Devices** app from the Microsoft Store. iTunes also
   works.
2. Open the app one time, then close it.

The service listens on `127.0.0.1:27015`. The setup wizard can open the
Microsoft Store page for you.

### macOS

Nothing to install. `usbmuxd` is part of macOS.

### Linux

1. Install `usbmuxd`:

   ```sh
   sudo apt install usbmuxd     # Debian, Ubuntu
   sudo pacman -S usbmuxd       # Arch
   ```

2. Make sure that the `usbmuxd` service runs.

## 2. Run the setup wizard

1. Connect the iPhone to the computer with the USB cable.
2. Unlock the iPhone.
3. Run:

   ```sh
   agentphone setup ios
   ```

The wizard examines each item in order and offers a fix for each problem. The
next sections tell what happens at each step.

## 3. Trust the computer

1. When the wizard asks "Ask the iPhone to trust this computer now?", answer yes.
2. On the iPhone, tap **Trust** in the "Trust This Computer?" alert.
3. Enter the passcode on the iPhone.

If no alert shows, disconnect the cable and connect it again.

## 4. Turn on Developer Mode

The Developer Mode switch shows only after the iPhone trusts the computer. The
wizard can make the switch visible. The path is the same on iOS 17, 18, and 26.

1. On the iPhone, open **Settings > Privacy & Security**.
2. Scroll to the bottom and tap **Developer Mode**.
3. Turn on **Developer Mode**.
4. Tap **Restart**. The iPhone restarts.
5. Unlock the iPhone.
6. Tap **Turn On** in the alert, then enter the passcode.

agentphone cannot turn on Developer Mode for you when the iPhone has a passcode.

## 5. Mount the developer disk image

The iPhone needs the Apple developer disk image (DDI) to accept touches from
the computer.

1. When the wizard asks "Download and install it now?", answer yes.
2. Wait for the download and the upload to complete.

agentphone downloads the DDI one time (about 20 MB) into its cache folder. The
iPhone removes the DDI at each restart. After a restart, run
`agentphone setup ios` again. The first mount after each restart needs Internet,
because Apple must sign the image for your iPhone.

## 6. Optional: use Wi-Fi

1. Keep the cable connected.
2. When the wizard asks "Also use the iPhone over Wi-Fi when the cable is
   unplugged?", answer yes.
3. Disconnect the cable.
4. Run `agentphone devices` and make sure that the iPhone is in the list.

The iPhone and the computer must be on the same network. Wi-Fi discovery uses
mDNS. If mDNS fails, agentphone tries the last known IP address. The Wi-Fi
path is unverified.

## 7. Make sure it works

```sh
agentphone doctor
agentphone screenshot -o screen.png
agentphone tap 0.5 0.5
```

## Files that agentphone keeps

| File | Location on Windows | Content |
|---|---|---|
| Pairing files | `%APPDATA%\agentphone\data\ios-pairing\` | `<udid>.plist`, `<udid>.rppairing`, `<udid>.json` |
| DDI cache | `<cache folder>\ddi\Xcode_iOS_DDI_Personalized\` | `BuildManifest.plist`, `Image.dmg`, `Image.dmg.trustcache` |

> CAUTION: Keep the pairing files private. A person with these files can
> control your iPhone. Set `ios.pairing_dir` to keep them in a different folder.

To use a different DDI mirror, set the environment variable
`AGENTPHONE_DDI_MIRROR` to a base URL that ends with `/`.

## Troubleshooting

`agentphone doctor` shows one line for each check. The table uses the check
names from `crates/ap-ios/src/setup.rs` and the names that doctor shows.

| Check id | Doctor name | Problem | Fix |
|---|---|---|---|
| `usbmuxd` | Apple Mobile Device service | The service does not answer. | Windows: install the Apple Devices app or iTunes. macOS: connect the cable, tap Trust, or restart the Mac. Linux: install and start `usbmuxd`. |
| `device` | iPhone connected | No iPhone on USB. | Connect a data cable. Unlock the iPhone. |
| `paired` | iPhone trusts this computer | The iPhone does not trust the computer. | Unlock the iPhone. Run `agentphone setup ios`. Tap Trust. |
| `ios_version` | iOS version | iOS is older than 17.4. | Update iOS in Settings > General > Software Update. |
| `developer_mode` | Developer Mode | Developer Mode is off. | Do [step 4](#4-turn-on-developer-mode). |
| `ddi` | Developer disk image | The DDI is not mounted. | Run `agentphone setup ios`. Do this after each restart. |
| `tunnel` | Secure connection | The CoreDevice tunnel does not open. | Unlock the iPhone. Connect the cable again. Run doctor again. |
| `hid_services` | Touch and keyboard input | The iPhone does not have the HID services. | Mount the DDI again. If the services stay missing, agentphone cannot control this iOS version. |
| `wifi_pairing` | Wi-Fi pairing | No Wi-Fi pairing is saved (warning only). | Do [step 6](#6-optional-use-wi-fi) with the cable connected. |

When one check fails, doctor does not show the checks after it. Fix the first
problem, then run `agentphone doctor` again.

### Known limits

- Typing uses a US keyboard layout. For text with accents, other scripts, or
  emoji, agentphone puts the text on the pasteboard and sends Cmd+V. This
  fallback is unverified.
- On iOS 17.4 to 18, the HID services can be missing. iOS 26 is the main
  target of the input code.
- A later iOS version can refuse the display service that agentphone opens
  for input.
