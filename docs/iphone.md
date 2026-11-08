# iPhone setup

This guide sets up an iPhone for agentphone. The steps are the same for all
supported iPhone models. The examples use an iPhone 13.

agentphone does not need a Mac, Xcode, an app on the iPhone, or administrator
rights. It needs a USB cable one time. After the setup, it can also use Wi-Fi.

> NOTE: Nobody tested agentphone on a real iPhone yet. If a step fails, run
> `agentphone doctor` and see [Troubleshooting](#troubleshooting).

## Requirements

- An iPhone with iOS 17.4 or later. iOS 17.3 and earlier are not supported.
  Touch input (tap, swipe, pinch) needs an iOS 27 release build or later.
  Early iOS 27 betas list no HID service. Below iOS 27, the iPhone refuses
  the display stream with CoreDevice error 9021 (reported on iOS 18 and 26,
  expected on 17.x). agentphone then works in "screen only" mode:
  screenshots, hardware buttons, and keys can work, but touch calls stop
  with the error "Touch input needs iOS 27 or later. This iPhone has iOS X.
  Update the iPhone in Settings > General > Software Update." Public reports
  say that iOS 26 accepts buttons without the stream. This is unverified on
  agentphone.
- Models newer than `iPhone18,5` can need a Cryptex1 developer disk image
  for input. agentphone does not support Cryptex1 yet.
- iPhone XS, XS Max, and XR stop at iOS 18. agentphone connects to them in
  screen only mode. Touch input does not work, because these models cannot
  get iOS 27.
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
mDNS. agentphone finds the iPhone by its Wi-Fi MAC address, or by the
`authTag` of the pairing. If mDNS fails, agentphone tries the last known IP
address. On a Wi-Fi reconnect, agentphone uses only the saved pairing. It
never starts a new pairing. The Wi-Fi path is unverified.

## 7. Make sure it works (first run)

1. Unlock the iPhone and go to the home screen.
2. Run:

   ```sh
   agentphone selftest
   ```

3. When agentphone asks "Start the input test?", type `y`. The default is no.
4. Send `selftest/selftest-report.json` to the developer.

The self-test does these steps in order:

1. It runs the `agentphone doctor` checks. If a check fails, the test stops and
   shows the fix.
2. It connects to the iPhone and shows the model, the iOS version, and the
   screen size.
3. It saves a screenshot in the `selftest` folder. Use `--out DIR` to change
   the folder.
4. Only after you type `y`, it presses Home, swipes one time from left to
   right on the home screen, and presses Home again. After each action, it
   compares a new screenshot with the previous one. Only a big change counts:
   a clock tick or a live widget does not. It does not type text and does
   not tap.
   - Touch input needs iOS 27 or later. On iOS below 27, the test skips these
     three steps and gives the reason.
   - The swipe is sent only when there is proof that the home screen shows:
     Home changed the screen, or you answer yes to the question "Is the home
     screen showing on the phone now?". Without proof, the test skips the
     swipe. If Home fails, the test skips the swipe too.
   - If the second Home does not change the screen, the step passes. The
     swipe can end on the first home page, where Home does nothing.

Each step shows PASS, FAIL, or SKIP, one reason, and one fix. The test also
writes `selftest-report.txt`. The report contains no secrets and no phone
names. It shows only the last 4 characters of a UDID or a serial. Press Ctrl+C to stop the test. The report
then keeps the results until that point.

`--yes` starts the input test without the questions. With `--yes`, the
swipe is sent only when Home changes the screen. To get this change, open an
app (for example Settings) before you start. Without a terminal and without
`--yes`, the input test is skipped. The exit code is 0 when all steps
that ran pass, and 1 when a step fails or you press Ctrl+C.

For a single manual test, use these commands:

```sh
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

On macOS and Linux, agentphone makes the folder with mode 700 and each file
with mode 600. On Windows, the files get the access rules of the folder. The
default folder is private to your user. If you set `ios.pairing_dir`, use a
folder that only your user can read.

To use a different DDI mirror, set the environment variable
`AGENTPHONE_DDI_MIRROR` to a base URL that ends with `/`. The DDI download
stops when the connection takes more than 15 seconds, or when no data comes
for 60 seconds.

## Time limits and retries

- Each iPhone call has a time limit. The limit is 30 seconds for the setup,
  plus the expected time of the work, plus 10 seconds. If the limit expires,
  agentphone closes the session and returns a time-out error. The next call
  opens a new session.
- agentphone retries a screenshot, the pasteboard, and an app launch one time
  after a connection error.
- agentphone sends input again only when the error came before the first
  event reached the iPhone. After that, it does not retry. The error says
  "input may be partly delivered". Look at the screen before you try again.
- If a gesture or a key stroke stops with an error, agentphone tries to
  release the touch or the key.

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
| `hid_services` | Touch and keyboard input | Fail: the iPhone does not have the HID services. Warning: iOS is older than 27, so touch input does not work (screen only). | Fail: mount the DDI again. If the services stay missing, agentphone cannot control this iOS version. Warning: update the iPhone in Settings > General > Software Update. |
| `wifi_pairing` | Wi-Fi pairing | No Wi-Fi pairing is saved (warning only). | Do [step 6](#6-optional-use-wi-fi) with the cable connected. |

When one check fails, doctor does not show the checks after it. Fix the first
problem, then run `agentphone doctor` again.

### Known limits

- Typing uses a US keyboard layout. For text with accents, other scripts, or
  emoji, agentphone puts the text on the pasteboard and sends Cmd+V. This
  fallback is unverified.
- Touch input needs iOS 27 or later. Below iOS 27, the iPhone refuses the
  display stream with CoreDevice error 9021 (reported on iOS 18 and 26,
  expected on 17.x). This includes every iPhone XS, XS Max, and XR, because
  they stop at iOS 18. On these versions:
  - The `hid_services` check is a warning with the update fix.
  - The device capabilities have no `tap` and no `swipe`. `agentphone info`
    and the MCP `status` tool show "screen only" and the reason.
  - A touch call stops at once with the update message. It sends nothing.
  - `agentphone run` stops before the first AI model call, because the agent
    cannot do a task without touch.
  - Buttons and keys go to the IndigoHid service without the stream. This
    is unverified.
- Models newer than `iPhone18,5` can need a Cryptex1 developer disk image.
  agentphone does not support Cryptex1 yet, so input can fail on these
  models.
- iOS has no query for the open app on this route. agentphone knows the app
  only right after `open_app`. After any input, the app is unknown.
- A later iOS version can refuse the display service that agentphone opens
  for input.
