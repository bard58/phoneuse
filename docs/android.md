# Android setup

This guide sets up an Android phone for agentphone. agentphone uses `adb`
(Android Debug Bridge). Any Android phone with USB debugging can work.

> NOTE: Nobody tested agentphone on a real Android phone yet. If a step fails,
> run `agentphone doctor` and see [Troubleshooting](#troubleshooting).

## Requirements

- An Android phone with USB debugging.
- A USB data cable for the first connection. Wi-Fi needs Android 11 or later.
- Google platform-tools (`adb`) on the computer.

## 1. Install platform-tools

### Windows

```powershell
winget install Google.PlatformTools
```

### macOS and Linux

Download platform-tools from
<https://developer.android.com/tools/releases/platform-tools>, or use the
package manager (for example `brew install android-platform-tools` or
`sudo apt install adb`).

agentphone finds `adb` on `PATH` and in the usual SDK folders. If it does not
find `adb`, set the path:

```sh
agentphone config set android.adb_path "D:\Android\platform-tools"
```

## 2. Turn on USB debugging

1. On the phone, open **Settings > About phone**. On Samsung, open
   **Settings > About phone > Software information**.
2. Tap **Build number** 7 times.
3. Open **Settings > System > Developer options**.
4. Turn on **USB debugging**.

The menu names can be different on some phone brands.

## 3. Connect the phone

1. Connect the phone with the USB cable.
2. Unlock the phone.
3. Run `agentphone setup android`.
4. On the phone, the alert "Allow USB debugging?" shows. Select
   **Always allow from this computer**, then tap **Allow**.

If no alert shows, disconnect the cable and connect it again.

## 4. Optional: use Wi-Fi

The phone and the computer must be on the same network.

1. On the phone, open **Developer options > Wireless debugging** and turn it on.
2. Tap **Pair device with pairing code**. The phone shows an address and a
   6-digit code.
3. In the wizard, select "Connect over Wi-Fi instead", or answer yes to the
   Wi-Fi question.
4. Type the pairing address (IP:port under the code).
5. Type the 6-digit code.
6. On the Wireless debugging screen, read the **IP address & Port**. Type it in
   the wizard.

You can also pair with `adb` directly:

```sh
adb pair 192.168.1.20:37000
adb connect 192.168.1.20:41000
```

## 5. Make sure it works

```sh
agentphone doctor
agentphone screenshot -o screen.png
agentphone tap 0.5 0.5
```

## Troubleshooting

The table uses the check ids from `agentphone doctor --json`.

| Check id | Doctor name | Problem | Fix |
|---|---|---|---|
| `adb` | adb (platform-tools) | `adb` is not found. | Install platform-tools. Put it on `PATH`, or set `android.adb_path`. |
| `adb_server` | adb server | The adb server does not start. | Run `adb start-server`. |
| `android_unauthorized` | Phone `<serial>` | The phone did not allow USB debugging. | Unlock the phone. Accept "Allow USB debugging?". If no alert shows, revoke the USB debugging authorizations in Developer options and connect the cable again. |
| `android_offline` | Phone `<serial>` | The phone is offline. | Unlock the phone and connect the cable again, or run `adb reconnect`. |
| `android_permissions` | Phone `<serial>` | No USB permission. | Windows: install the Google USB driver. Linux: fix the udev rules. Then connect the cable again. |
| `android_other` | Phone `<serial>` | The phone is in a different state. | Start the phone normally and turn on USB debugging. |

When you use only an iPhone, the Android checks show as warnings. This is
correct.

On Windows, agentphone starts the adb server in the background through
PowerShell. If this fails, the error tells you to run `adb start-server` one
time in a terminal. Then try again. If the adb server stops later, agentphone
starts it again at the next connection.

For an IPv6 adb server address in `android.adb_host`, you can write the
address with or without brackets, for example `[::1]` or `::1`.

### Coordinates

agentphone uses fractions of the screen (0 to 1) for all positions. On
Android, the screen size in points is the size in dp.

agentphone reads the screen rotation when it opens the phone. In landscape,
it swaps the width and the height. If the phone rotates later, agentphone
uses the new size after the next screenshot.

### Known limits

- A swipe, a long press, or a drag lasts at most 30 seconds. agentphone
  shortens a longer duration to 30 seconds.
- Long text with ADBKeyBoard goes in parts of at most 2202 bytes. If a part
  fails, the error tells how many characters are already typed.
- Shift with a digit or a symbol gives the US-layout character, for example
  Shift+1 gives `!`.
- When the notification shade or the lock screen has the focus, the open app
  is unknown. agentphone does not detect heads-up notifications.
- Jev and the safety checks see the package id of the app, not its name.
