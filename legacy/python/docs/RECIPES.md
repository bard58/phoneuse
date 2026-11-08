# Operating the iPhone with `phoneuse`

`phoneuse up` runs the AirPlay mirror and mouse control together; on the iPhone,
start Control Center → Screen Mirroring → PhoneUse. Agents then use the commands
below and look at `phoneuse screenshot`, which waits for the mirror to catch up
after the last action, so it always shows the result.

Coordinates are fractions of the screenshot (0 to 1), or iPhone points when any
value is above 1. The screen is 402 × 874 points.

## Loop for an agent

Take a screenshot, find the target, act, take a screenshot again. Taps land
exactly where asked, so there is no need to aim twice. Swipes land within a few
points, which is plenty for scrolling and edge gestures.

## Recipes (verified on an iPhone 17 Pro, iOS 26, November 2026)

| Goal | Command | What it does |
|---|---|---|
| Tap | `phoneuse tap X Y` | press and release at the point |
| Long press | `phoneuse longpress X Y [S]` | hold 0.8 s by default; opens context menus |
| Home | `phoneuse home` | Cmd+H; works from any app |
| Back | `phoneuse back` | swipe right starting at the very left edge (x = 0), like a finger sliding in |
| App switcher | `phoneuse switcher` | slow swipe up from the bottom, then hold |
| Control Center | `phoneuse control-center` | slow (0.6 s) swipe down from the top-right corner |
| Notification Center | `phoneuse notifications` | slow swipe down from the top-left |
| Close an overlay | `phoneuse dismiss` | fast, long swipe up from the very bottom; closes Control Center, Notification Center and the switcher |
| Scroll | `phoneuse scroll down [N]` / `up` | drags the content by N half-screens |
| Open an app | `phoneuse open Settings` | Cmd+H, Cmd+Space, type the name, Enter |
| Type | `phoneuse type "hello\n"` | US layout; `\n` presses Enter |
| Keys | `phoneuse key cmd+space` | modifiers: cmd, shift, alt, ctrl |

## Things that don't work, or bite

- Avoid tapping the top centre of the screen: it hits the Dynamic Island, which
  opens the Screen Mirroring popup with a Stop Mirroring button.
- Cmd+H doesn't close Control Center or Notification Center; use `phoneuse dismiss`.
- iOS ignores the mouse wheel and Cmd+Tab. Over the mirror window and in the
  browser mirror, phoneuse turns the wheel into a drag instead (see pointer.py).
- Fast swipes down from the top are ignored; the top-edge recipes are slow on purpose.
- If the phone locks, the passcode screen appears. The agent must stop and ask the
  owner to unlock it. Never type a passcode.

## Why it works this way (HID facts measured on the device)

- The absolute pointer report lands exactly where asked, anywhere on the screen.
- Button presses count only from the absolute pointer, not from the relative mouse.
- While the button is held, an absolute position change ends the touch after about
  90 ms (it reads as a click), so a drag is an absolute press, then relative motion
  (button 0), then an absolute release at the same spot. Verified with touchlab/.
- Move the pointer, wait about 0.1 s, then press. Pressing in the same instant as
  a jump makes iOS start the touch on the way, so an edge swipe turns into a drag
  of the page (this is what broke `back` at first).
- iOS accelerates relative motion by report size and drops motion from reports
  that arrive less than about 8 ms apart, so drags send one report every 8 ms
  (including empty ones, which keeps slow drags alive), sized from a measured
  table (`touch.DRAG_PTS`) and rounded down so the touch never overshoots.

## iPhone settings this relies on

- Accessibility → Touch → AssistiveTouch: on; Dwell Control: off (otherwise a
  resting pointer taps on its own).
- Optional: Accessibility → Accessories → Pointer Control colour Orange, large size, Automatically
  Hide Pointer off. Only `phoneuse where` and the telemetry tools use it, to find
  the pointer in screenshots; gestures never need to locate the pointer.
- Consider Display & Brightness → Auto-Lock: Never while an agent is working.
