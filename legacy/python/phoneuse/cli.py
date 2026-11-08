#!/usr/bin/python
"""phoneuse: see and operate an iPhone from a Linux PC.

Setup and running:
  phoneuse install              one-time: Bluetooth HID service (sudo), bluetoothd
                               config, and the local vision models
  phoneuse up                   mirror window + mouse control (click/drag on the mirror)
  phoneuse status               Bluetooth link and mirror window
  phoneuse done                 end the agent session: control goes back to the user
  phoneuse unlock               on the lock screen: ask the owner to approve entering the
                               passcode they saved in the browser mirror's settings
  phoneuse view [--print]       open the browser mirror (http://localhost:PORT) in the
                               default browser; --print only prints its address

Agent commands. Coordinates are fractions (0..1) of the screenshot, or phone
points if any value is > 1 (screen_points in ~/.config/phoneuse/config.toml):
  phoneuse screenshot [PATH]    PNG of the phone screen, once it has stopped changing
  phoneuse tap X Y              tap
  phoneuse longpress X Y [S]    press and hold (default 0.8 s)
  phoneuse swipe X0 Y0 X1 Y1 [S]  drag from one point to another over S seconds
  phoneuse scroll up|down [N]   scroll the content by N half-screens (default 1)
  phoneuse home | back | switcher | control-center | notifications | dismiss
  phoneuse open APP             open an app by name (search, type, Enter)
  phoneuse type [--append] TEXT  type text (US layout); \\n presses Enter. Replaces what
                               the field holds (Cmd+A first) unless --append
  phoneuse key COMBO            e.g. enter, escape, cmd+h, cmd+space, shift+tab
  phoneuse keyboard             show or hide the iPhone's on-screen keyboard (Eject key)
  phoneuse where                debugging: where the pointer appears (needs Pointer
                               Control colour Orange)

Any action command can end with --text to print the screen as text afterwards
(as `phoneuse text`), e.g. `phoneuse tap 0.3 0.5 --text`.

Reading the screen without images (local models; a few hundred tokens instead
of ~1,300 for a screenshot):
  phoneuse text [--json] [--all] every line of text on screen with its centre (x, y), and
                               on/off switches as [switch on] / [switch off]; --all keeps
                               the status bar and stray icon glyphs
  phoneuse find WHAT            where an element is, from a plain description
                               ("clear search (X) button", "Settings app icon")
  phoneuse tap-on WHAT          find it and tap it; prints where it tapped

iPhone settings this relies on: AssistiveTouch on, Dwell Control off.
"""
import os
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

from . import screen, touch
from .config import CFG, NAME, SCREEN_PT, VISION_SOCKET
from .hidclient import Hid

PKG = os.path.dirname(os.path.realpath(__file__))
ROOT = os.path.dirname(PKG)  # the repository checkout

PT = SCREEN_PT
LAST_ACTION = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "phoneuse-last-action")
# UxPlay receives AirPlay and passes the phone's H.264 stream on as RTP to the
# video hub (hub.py), which serves the mirror window and the browser mirror.
MIRROR_ARGS = ["uxplay", "-p", "-n", NAME, "-nh",  # -nh: no "@hostname" after the name
               "-vrtp", f"config-interval=1 pt=96 mtu=60000 ! udpsink host=127.0.0.1 port={screen.HUB_RTP_PORT} sync=false",
               "-vsync", "no", "-s", f"{SCREEN_PT[0] * 3}x{SCREEN_PT[1] * 3}@60", "-fps", "60",
               # audio: mirrored audio already plays unsynced (-vsync no); keep the sink's
               # buffer at 40 ms (pulsesink default is up to 200 ms) and ask the phone to
               # plan for 0.1 s instead of 0.25 s
               "-as", "pulsesink buffer-time=40000 latency-time=10000", "-al", "0.1"]


def die(msg):
    print(msg, file=sys.stderr)
    sys.exit(1)


def point(xs, ys):
    """Parse a coordinate pair: fractions, or iPhone points if either is > 1."""
    x, y = float(xs), float(ys)
    if x > 1 or y > 1:
        x, y = x / PT[0], y / PT[1]
    if not (0 <= x <= 1 and 0 <= y <= 1):
        die(f"coordinate out of range: {xs} {ys}")
    return x, y


def need_mirror():
    """Stop unless the mirror shows the phone: acting or reading blind is unsafe."""
    if not screen.mirror_live():
        die("The mirror isn't showing the phone, so nothing was done. Start Screen Mirroring "
            f"to {NAME} on the iPhone (Control Center > Screen Mirroring), then check `phoneuse status`.")


LAST_SEEN = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "phoneuse-last-seen")
NOTES = []  # remarks for the agent, printed (CLI) or returned (MCP) with the action


def agent_turn():
    """Before an agent action: tell the browser mirror an agent is driving (it
    glows and locks its own input), and stop if the user has taken over. Also
    notes when the screen changed since the agent last looked without it acting
    (the user may be using the phone, or something is animating)."""
    import json
    req = urllib.request.Request(f"http://127.0.0.1:{CFG['web_port']}/api/agent/act", data=b"{}", method="POST",
                                 headers={"Content-Type": "application/json"})
    try:
        urllib.request.urlopen(req, timeout=2).close()
    except urllib.error.HTTPError as e:
        if e.code == 409:
            die(json.loads(e.read()).get("result", "the user took over the phone; stop and wait"))
    except OSError:
        pass  # no browser mirror running: nothing to tell
    try:
        seen_at = os.path.getmtime(LAST_SEEN)
        acted_at = os.path.getmtime(LAST_ACTION) if os.path.exists(LAST_ACTION) else 0
        # only when the agent looked after its own last action (else its own frames count)
        if time.time() - seen_at < 600 and seen_at >= acted_at:
            seen, now = int(open(LAST_SEEN).read()), frame_seq()
            if now is not None and now > seen + 1:  # one late change is usually the end of an animation
                NOTES.append("note: the screen changed since your last look without an action from you "
                             "(someone may be using the phone, or it is animating); look before relying on it")
    except (OSError, ValueError):
        pass


def hid():
    """The Bluetooth link, for commands that act on the phone."""
    need_mirror()
    agent_turn()
    try:
        h = Hid()
    except OSError:
        die("Bluetooth HID service isn't running. Run: phoneuse install")
    st = h.cmd("status")
    if "connected=True" not in st:
        try:
            h.cmd("connect")
        except RuntimeError as e:
            die(f"iPhone not connected over Bluetooth ({e}). On the phone: Settings > Bluetooth > {NAME}.")
    return AgentHid(h)


class AgentHid:
    """The Bluetooth link for agent actions: also shows where the agent points on
    the browser mirror (a glowing dot), from the absolute pointer commands."""

    def __init__(self, h):
        self.h = h
        self.url = f"http://127.0.0.1:{CFG['web_port']}/api/agent/cursor"
        self.first_seq = None  # picture-change count just before the action's first command

    def cmd(self, line):
        if self.first_seq is None and line != "status":
            # Taken here, after any OCR or grounding lookup the action did first:
            # changes during a lookup must not count as this action's result.
            self.first_seq = frame_seq()
        r = self.h.cmd(line)
        if line.startswith("a "):
            try:
                _, x, y, b = line.split()[:4]
                body = f'{{"x": {float(x)}, "y": {float(y)}, "down": {str(b != "0").lower()}}}'.encode()
                req = urllib.request.Request(self.url, data=body, method="POST", headers={"Content-Type": "application/json"})
                urllib.request.urlopen(req, timeout=0.3).close()
            except (OSError, ValueError):
                pass  # the dot is a courtesy; never slow or fail the action
        return r


def frame_seq():
    """The count of picture changes (frames service; None without it): taken
    before an action, so a later read can wait for that action's result."""
    try:
        line, _ = screen.frames_request("status", timeout=1)
        return int(line.split("seq=")[1].split()[0])
    except (OSError, IndexError, ValueError):
        return None


def acted(seq_before=None):
    """Note that an action happened, and the frame count from before it."""
    with open(LAST_ACTION, "w") as f:
        f.write("" if seq_before is None else str(seq_before))


def settle():
    """Wait until the phone's screen is still: after a recent action, until its
    first frame has arrived and frames have stopped; otherwise at once. Driven
    by the hub's frame clock, no fixed delays."""
    since = None
    try:
        if time.time() - os.path.getmtime(LAST_ACTION) < 5:
            txt = open(LAST_ACTION).read().strip()
            since = int(txt) if txt else None
    except (FileNotFoundError, ValueError):
        pass
    try:
        line, _ = screen.frames_request(f"settle {since}" if since is not None else "settle", timeout=6)
        with open(LAST_SEEN, "w") as f:  # what the agent has now seen (see agent_turn)
            f.write(line.split("seq=")[1].split()[0])
        note = None
        if "still=0" in line:
            note = "note: the screen was still changing when read; look again if it matters"
        elif "ambient=1" in line or float(line.split("ignored=")[1].split()[0]) > 0.05:
            note = "note: parts of the screen keep moving (video or animation); read the rest as settled"
        if note and note not in NOTES:
            NOTES.append(note)
        if since is not None:  # this action's result has been waited for: later reads needn't wait again
            acted(None)
    except (OSError, IndexError):
        time.sleep(0.3)  # no hub (older setup): a short pause instead


def key_combo(h, combo):
    parts = combo.lower().replace("-", "+").split("+")
    mods = {"cmd": "gui", "command": "gui", "ctrl": "ctrl", "control": "ctrl", "alt": "alt",
            "option": "alt", "opt": "alt", "shift": "shift", "gui": "gui"}
    key = parts[-1]
    h.cmd("key " + " ".join([key] + [mods[p] for p in parts[:-1]]))


# ---------- gestures (recipes verified on iOS 26, see RECIPES.md) ----------

def home(h):
    h.cmd("key h gui")


def back(h):
    # finger sliding in from the very left edge (touch.press settles the pointer first)
    touch.swipe(h, 0.0, 0.5, 0.8, 0.5, duration=0.35)


def switcher(h):
    touch.swipe(h, 0.5, 1.0, 0.5, 0.62, duration=0.45, hold_end=0.6)


def control_center(h):
    touch.swipe(h, 0.88, 0.0, 0.88, 0.45, duration=0.6)


def notifications(h):
    touch.swipe(h, 0.15, 0.0, 0.15, 0.7, duration=0.6)


def dismiss(h):
    """Close Control Center, Notification Center, the switcher, or go home from an app."""
    touch.swipe(h, 0.5, 1.0, 0.5, 0.15, duration=0.12)


def scroll(h, direction, n=1.0):
    """Drag the content by n half-screens. (Resting the finger before lifting
    didn't stop iOS's fling, and the pause read as a settled screen: a stale
    read. Settle waits out the fling instead, ~1 s.)"""
    d = 0.4 * n
    y0, y1 = (0.3, 0.3 + d) if direction == "up" else (0.7, 0.7 - d)
    touch.swipe(h, 0.5, y0, 0.5, min(max(y1, 0.05), 0.95), duration=0.35)


def steplog(*a):
    if os.environ.get("PHONEUSE_STEPLOG"):
        print(f"{time.time():.3f}", *a, file=sys.stderr, flush=True)


def step(h, command, quiet=0.1, change=0.6):
    """Send one HID command and wait until the phone has shown its result (the
    frames service), instead of a fixed pause. `quiet`: how long the picture
    must stay still, longer where results stream in (search)."""
    seq = frame_seq()
    steplog("send", command)
    h.cmd(command)
    if seq is None:
        time.sleep(0.8)  # no frames service (older setup)
        return
    try:
        screen.frames_request(f"settle {seq} {quiet} {change}", timeout=6)
        acted(None)  # settled: reads in the same command needn't wait for a change again
        steplog("settled", command)
    except OSError:
        time.sleep(0.8)


def refuse_if_locked():
    """Stop before typing into the lock screen: typed text and Enter there would
    go to the passcode field, and a submitted wrong passcode counts as a failed
    attempt. Only the owner may unlock the phone."""
    from . import unlock
    if unlock.locked(screen_text):  # the passcode prompt, or the lock screen's clock and date
        die("The phone is locked (passcode screen). Nothing was typed. Run `phoneuse unlock` to "
            "ask the owner to approve an unlock, or ask them to unlock it.")


def open_app(h, name):
    step(h, "key h gui", change=0.3)  # often already home (no change); also wakes a locked phone
    refuse_if_locked()
    step(h, "key space gui")
    step(h, "type " + name, quiet=0.35)  # search results arrive in waves
    step(h, "key enter", quiet=0.3)  # wait for the app to come up


def type_text(h, text):
    refuse_if_locked()
    for i, chunk in enumerate(text.replace("\\n", "\n").split("\n")):
        if i:
            h.cmd("key enter")
        if chunk:
            h.cmd("type " + chunk)


# ---------- screen ----------

def screenshot(path):
    need_mirror()
    settle()
    try:
        png = screen.grab_png()
    except RuntimeError:
        die(f"No mirror window. Run `phoneuse up` and start Screen Mirroring to '{NAME}' on the iPhone.")
    with open(path, "wb") as f:
        f.write(png)
    from PIL import Image
    import io
    w, hgt = Image.open(io.BytesIO(png)).size
    print(f"{path} ({w}x{hgt} px = {PT[0]}x{PT[1]} pt; tap with fractions of this image or points)")


# ---------- vision (OCR + grounding, see vision.py) ----------

VISION_SOCK = VISION_SOCKET


def vision(req, timeout=90):
    import json
    import socket

    def ask():
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(timeout)
        s.connect(VISION_SOCK)
        s.sendall((json.dumps(req) + "\n").encode())
        reply = json.loads(s.makefile().readline())
        s.close()
        return reply

    try:
        return ask()
    except (FileNotFoundError, ConnectionRefusedError):
        pass
    py = os.path.join(ROOT, ".venv-vision", "bin", "python")
    if not os.path.exists(py):
        die(f"Vision models aren't installed. Run: {ROOT}/scripts/setup-vision.sh")
    logf = open(os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "phoneuse-vision.log"), "a")
    subprocess.Popen([py, "-m", "phoneuse.vision"], cwd=ROOT, stdout=logf, stderr=logf, start_new_session=True)
    for _ in range(120):
        time.sleep(0.5)
        try:
            return ask()
        except (FileNotFoundError, ConnectionRefusedError):
            continue
    die("vision service didn't start; see $XDG_RUNTIME_DIR/phoneuse-vision.log")


def fresh_screenshot():
    """A screenshot for reading, once the screen has settled (see settle)."""
    path = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), f"phoneuse-shot-{os.getpid()}.png")
    import contextlib, io
    with contextlib.redirect_stdout(io.StringIO()):
        screenshot(path)
    return path


STATUS_BAR = 0.055  # lines above this (time, signal, battery) are left out of `text`
BACK_GLYPHS = {"く", "<", "‹", "〈", "〈"}  # how OCR reads the back chevron at the top left


def screen_text(show_all=False, shot=None):
    """The screen as text lines [(x, y, text)]: OCR plus the state of on/off
    switches, which OCR can't read. Drops the status bar and icon glyphs OCR
    mistakes for text (lines without a letter or digit) unless show_all. Takes a
    settled screenshot unless given one (`shot`, a PNG path, which it leaves)."""
    import numpy as np
    from PIL import Image
    own = shot is None
    shot = shot or fresh_screenshot()
    r = vision({"op": "text", "image": shot})
    rgb = np.asarray(Image.open(shot).convert("RGB"))
    if own:
        os.unlink(shot)
    if "error" in r:
        die(r["error"])
    lines = [(ln["x"], ln["y"], "[back]" if ln["text"].strip() in BACK_GLYPHS and ln["x"] < 0.15 else ln["text"])
             for ln in r["lines"]
             if show_all or (ln["y"] >= STATUS_BAR and any(c.isalnum() for c in ln["text"]))]
    lines += [(x, y, "[switch on]" if on else "[switch off]") for x, y, on in screen.find_switches(rgb)]
    return sorted(lines, key=lambda t: (round(t[1] / 0.012), t[0]))  # rows top to bottom, then left to right


def text_cmd(args):
    import json
    lines = screen_text("--all" in args)
    if "--json" in args:
        print(json.dumps([{"x": x, "y": y, "text": t} for x, y, t in lines]))
        return
    for x, y, t in lines:
        print(f"{x:.3f} {y:.3f}  {t}")
    if NOTES:
        print("\n".join(NOTES))


FILLER = {"row", "button", "icon", "link", "tab", "cell", "item", "the", "label", "option"}
# Descriptions of things that aren't text; only these fall back to the grounding model.
VISUAL = {"icon", "button", "image", "picture", "logo", "symbol", "glyph", "arrow", "switch",
          "toggle", "avatar", "photo", "thumbnail", "chevron", "x", "plus", "+", "gear", "badge"}


def text_target(what):
    """Position of an on-screen text line matching `what` (ignoring words like
    "row" or "button"), or None. Exact, unlike the grounding model, which always
    answers even when the element isn't on screen."""
    words = [w for w in what.lower().replace('"', "").split() if w not in FILLER]
    if not words:
        return None
    want = " ".join(words)
    lines = [(x, y, t.lower().strip()) for x, y, t in screen_text() if y >= STATUS_BAR]
    exact = [(x, y) for x, y, t in lines if t == want]
    if exact:
        return exact[0]
    starts = [(x, y) for x, y, t in lines if t.startswith(want)]
    if len(starts) == 1:
        return starts[0]
    # OCR misreads a character now and then ("Message" for "iMessage" when the
    # text cursor covers the i): take the single closest line if it is close.
    import difflib
    scored = sorted(((difflib.SequenceMatcher(None, want, t).ratio(), x, y) for x, y, t in lines), reverse=True)
    if scored and scored[0][0] >= 0.85 and (len(scored) == 1 or scored[1][0] < scored[0][0] - 0.1):
        return scored[0][1], scored[0][2]
    return None


def tap_on(h, what):
    """Tap an element by description: on-screen text exactly (OCR), else, for
    visual descriptions only, the grounding model. Returns what it did."""
    hit = text_target(what)
    if hit:
        x, y = hit
        how = "text"
    elif not VISUAL.intersection(what.lower().replace('"', "").split()):
        die(f"'{what}' isn't on screen as text, so nothing was tapped. Scroll to it, or describe "
            "something visual (\"gear icon\", \"back button\") to use the grounding model.")
    else:
        x, y = find(what)
        how = "grounding (check the result)"
        # find always answers, even when the element isn't there; it then tends to
        # point at the status bar. Never tap there: the Dynamic Island in its middle
        # opens the Stop Mirroring popup.
        if y < 0.06:
            die(f"refusing to tap the status bar at {x:.3f} {y:.3f}: '{what}' is probably not on screen")
    touch.tap(h, x, y)
    return f"tapped {x:.3f} {y:.3f} by {how}"


def find(what):
    shot = fresh_screenshot()
    r = vision({"op": "find", "image": shot, "what": what})
    os.unlink(shot)
    if "error" in r:
        die(f"not found: {what}")
    return r["x"], r["y"]


# ---------- up / install / status ----------

def up():
    if subprocess.run(["systemctl", "is-active", "--quiet", "phoneuse-hid"]).returncode != 0:
        die("Bluetooth HID service isn't running. Run: phoneuse install")
    print(f"Starting the mirror (AirPlay receiver '{NAME}'). On the iPhone: Control Center > Screen Mirroring > {NAME}.")
    stop = threading.Event()
    procs = {}

    def supervise(module, label, watch):
        """Run phoneuse.<module>; start it again when it exits or when one of the
        `watch` files changes. (The page itself is served fresh on every load.)"""
        files = [os.path.join(PKG, f) for f in watch]
        mtimes = lambda: [os.path.getmtime(f) for f in files if os.path.exists(f)]
        while not stop.is_set():
            seen = mtimes()
            procs[module] = subprocess.Popen([sys.executable, "-m", f"phoneuse.{module}"], cwd=ROOT)
            while procs[module].poll() is None and not stop.is_set():
                time.sleep(1)
                if mtimes() != seen:
                    print(time.strftime("%H:%M:%S"), f"{label} code changed; restarting it", flush=True)
                    procs[module].terminate()
                    procs[module].wait(5)
                    break
            else:
                if stop.is_set():
                    break
                print(time.strftime("%H:%M:%S"), f"{label} exited ({procs[module].returncode}); starting it again.", flush=True)
                time.sleep(1)

    def start_vision():
        # The screen-reading models (OCR and grounding) load as soon as phoneuse
        # starts, not on an agent's first look. Skipped if they aren't installed.
        if os.path.exists(os.path.join(ROOT, ".venv-vision", "bin", "python")):
            try:
                import contextlib, io
                with contextlib.redirect_stderr(io.StringIO()):
                    vision({"op": "ping"}, timeout=120)
            except SystemExit:
                pass
    threading.Thread(target=start_vision, daemon=True).start()

    def keep_hub():
        # The video hub (hub.py): decodes the phone's stream and serves it to the
        # window and the browser mirror. Started first; rarely restarts.
        while not stop.is_set():
            procs["hub"] = subprocess.Popen([sys.executable, "-m", "phoneuse.hub"], cwd=ROOT)
            rc = procs["hub"].wait()
            if stop.is_set():
                break
            print(time.strftime("%H:%M:%S"), f"video hub exited ({rc}); starting it again. "
                  "Reconnect Screen Mirroring on the iPhone.", flush=True)
            time.sleep(1)
    threading.Thread(target=keep_hub, daemon=True).start()

    def keep_frames():
        # The frames service (frames.py): settle and screenshots for agents. It
        # joins the hub as a viewer, so it restarts freely, also on code changes.
        supervise("frames", "frames service", ["frames.py", "screen.py"])
    threading.Thread(target=keep_frames, daemon=True).start()

    def keep_window():
        # The mirror window (window.py) outlives UxPlay restarts and phone
        # reconnects; it waits for the next stream by itself.
        while not stop.is_set():
            procs["window"] = subprocess.Popen([sys.executable, "-m", "phoneuse.window"], cwd=ROOT)
            rc = procs["window"].wait()
            if stop.is_set():
                break
            print(time.strftime("%H:%M:%S"), f"mirror window exited ({rc}); opening it again.", flush=True)
            time.sleep(1)
    threading.Thread(target=keep_window, daemon=True).start()

    def keep_web():
        # The browser mirror (web.py), on localhost only. Restarts when its code
        # changes: it rejoins the hub, so the phone keeps mirroring.
        supervise("web", "browser mirror", ["web.py", "pointer.py", "unlock.py", "screen.py"])
    threading.Thread(target=keep_web, daemon=True).start()

    def keep_mirror():
        # If UxPlay dies, start it again so the phone can reconnect without
        # restarting `phoneuse up`.
        while not stop.is_set():
            env = dict(os.environ)
            out = subprocess.DEVNULL
            if os.environ.get("PHONEUSE_TRACE"):  # per-element GStreamer latency, for tuning
                env.update(GST_TRACERS="latency(flags=pipeline+element)", GST_DEBUG="GST_TRACER:7")
                out = open(os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "phoneuse-gst-trace.log"), "w")
            procs["mirror"] = subprocess.Popen(MIRROR_ARGS, stdout=out, stderr=out, env=env)
            rc = procs["mirror"].wait()
            if stop.is_set():
                break
            print(time.strftime("%H:%M:%S"), f"mirror exited ({rc}); restarting it. Reconnect Screen Mirroring on the iPhone.", flush=True)
            time.sleep(1)
    threading.Thread(target=keep_mirror, daemon=True).start()
    try:
        h = Hid()
        print("Bluetooth:", h.cmd("status").replace("ok ", ""))
        print("Mouse control on: hover, click and drag on the mirror window. Ctrl+C stops everything.")
        print(f"Browser mirror: http://localhost:{CFG['web_port']}")
        # The clicker runs as its own process and restarts by itself when its code
        # changes, so edits don't drop the AirPlay connection.
        watched = [os.path.join(PKG, f) for f in ("mirror_input.py", "pointer.py", "touch.py", "screen.py", "hidclient.py")]
        mtimes = lambda: [os.path.getmtime(f) for f in watched]
        while not stop.is_set():
            seen = mtimes()
            procs["clicker"] = subprocess.Popen([sys.executable, "-m", "phoneuse.mirror_input"], cwd=ROOT)
            while procs["clicker"].poll() is None:
                time.sleep(1)
                if mtimes() != seen:
                    print(time.strftime("%H:%M:%S"), "clicker code changed; reloading it (mirror stays up)", flush=True)
                    procs["clicker"].terminate()
                    procs["clicker"].wait(3)
                    break
            else:
                print(time.strftime("%H:%M:%S"), "clicker exited; restarting it", flush=True)
                time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        if procs.get("clicker") and procs["clicker"].poll() is None:
            procs["clicker"].terminate()
        for name in ("mirror", "window", "web", "frames", "hub"):
            p = procs.get(name)
            if p and p.poll() is None:
                p.terminate()
                try:
                    p.wait(3)
                except subprocess.TimeoutExpired:
                    p.kill()


def status():
    svc = subprocess.run(["systemctl", "is-active", "phoneuse-hid"], capture_output=True, text=True).stdout.strip()
    print("HID service:", svc)
    if svc == "active":
        print("Bluetooth:", Hid().cmd("status").replace("ok ", ""))
    try:
        at, size = screen.mirror_window()
        live = "showing the phone" if screen.mirror_live() else "not mirroring (input to the phone is off)"
        print("Mirror window:", at, size, live)
    except RuntimeError:
        print(f"Mirror window: none (run `phoneuse up`, then Screen Mirroring > {NAME} on the iPhone)")


def view(print_only):
    """The browser mirror's address; opens it in the default browser unless print_only."""
    url = f"http://localhost:{CFG['web_port']}"
    try:
        urllib.request.urlopen(url, timeout=2).close()
    except OSError:
        die(f"The browser mirror isn't running at {url}. Start `phoneuse up`.")
    if not print_only:
        subprocess.Popen(["xdg-open", url], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print(url)


def agent_done():
    """End the agent's session: the browser mirror stops glowing and its mouse and
    keyboard reach the phone again, at once (not after 20 idle seconds)."""
    req = urllib.request.Request(f"http://127.0.0.1:{CFG['web_port']}/api/agent/done", data=b"{}", method="POST",
                                 headers={"Content-Type": "application/json"})
    try:
        urllib.request.urlopen(req, timeout=2).close()
    except OSError:
        pass
    print("ok: session ended; control is back with the user")


def unlock_cmd():
    """Ask the owner to unlock the phone with the saved passcode. Waits for their
    answer (up to 2 minutes); the passcode never passes through here."""
    import json
    req = urllib.request.Request(f"http://127.0.0.1:{CFG['web_port']}/api/unlock", data=b"{}", method="POST",
                                 headers={"Content-Type": "application/json"})
    print("Asked the owner to approve an unlock (browser mirror or notification); waiting...", flush=True)
    try:
        with urllib.request.urlopen(req, timeout=150) as r:
            print(json.loads(r.read())["result"])
    except urllib.error.HTTPError as e:
        die(json.loads(e.read()).get("result", str(e)))
    except OSError as e:
        die(f"The browser mirror isn't running ({e}). Start `phoneuse up`.")


def main():
    a = sys.argv[1:]
    if not a or a[0] in ("-h", "--help", "help"):
        print(__doc__)
        return
    cmd, args = a[0], a[1:]
    # `--text` at the end of an action: print the screen as text afterwards, saving
    # a second call (only as the last argument, so `type` can still type it).
    then_text = bool(args) and args[-1] == "--text" and cmd not in ("text", "find", "screenshot")
    if then_text:
        args = args[:-1]
    if cmd == "install":
        subprocess.run(["sudo", os.path.join(ROOT, "scripts", "install.sh")], check=True)
        if not os.path.exists(os.path.join(ROOT, ".venv-vision")):
            import shutil
            if shutil.which("uv"):
                subprocess.run([os.path.join(ROOT, "scripts", "setup-vision.sh")], check=True)
            else:
                print("skipping the screen-reading models (phoneuse text/find): install uv, then run scripts/setup-vision.sh")
        return
    if cmd == "up":
        return up()
    if cmd == "status":
        return status()
    if cmd == "unlock":
        return unlock_cmd()
    if cmd == "done":
        return agent_done()
    if cmd == "view":
        return view("--print" in args)
    if cmd == "text":
        return text_cmd(args)
    if cmd == "find":
        x, y = find(" ".join(args))
        print(f"{x:.3f} {y:.3f}")
        return
    if cmd == "screenshot":
        paths = [x for x in args if not x.startswith("--")]  # (--keep-pointer: now always)
        return screenshot(paths[0] if paths else os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "phoneuse.png"))
    h = hid()
    seq_before = frame_seq()
    if cmd == "tap":
        touch.tap(h, *point(*args[:2]))
    elif cmd == "longpress":
        touch.long_press(h, *point(*args[:2]), hold=float(args[2]) if len(args) > 2 else 0.8)
    elif cmd == "swipe":
        x0, y0 = point(*args[0:2]); x1, y1 = point(*args[2:4])
        touch.swipe(h, x0, y0, x1, y1, duration=float(args[4]) if len(args) > 4 else 0.3)
    elif cmd == "scroll":
        scroll(h, args[0] if args else "down", float(args[1]) if len(args) > 1 else 1.0)
    elif cmd in ("home", "back", "switcher", "dismiss", "notifications"):
        globals()[cmd](h)
    elif cmd == "control-center":
        control_center(h)
    elif cmd == "open":
        open_app(h, " ".join(args))
    elif cmd == "type":
        # The text replaces what the field holds (Cmd+A first): fields often keep
        # earlier text. --append adds to it instead.
        if args and args[0] == "--append":
            args = args[1:]
        else:
            key_combo(h, "cmd+a")
        type_text(h, " ".join(args))
    elif cmd == "key":
        key_combo(h, args[0])
    elif cmd == "keyboard":  # Eject: show or hide the on-screen keyboard
        h.cmd("eject")
    elif cmd == "tap-on":
        print(tap_on(h, " ".join(args)))
    elif cmd == "where":
        p = screen.find_orange(screen.grab_rgb())
        print(f"{p[0]:.3f} {p[1]:.3f}" if p else "not found")
        return
    else:
        die(f"unknown command: {cmd} (phoneuse --help)")
    acted(h.first_seq if getattr(h, "first_seq", None) is not None else seq_before)
    print("ok")
    if then_text:
        for x, y, t in screen_text():
            print(f"{x:.3f} {y:.3f}  {t}")
    if NOTES:
        print("\n".join(NOTES))


if __name__ == "__main__":
    main()
