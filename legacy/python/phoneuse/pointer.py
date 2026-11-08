"""Turn a pointer over the mirror into touches on the phone. Shared by the mirror
window (mirror_input.py) and the browser mirror (web.py).

Positions are fractions of the phone screen, (0, 0) top-left to (1, 1)
bottom-right. Positions outside 0..1 are on the bezel: a hover or press there
goes to the nearest screen edge, so pressing the bottom bezel and dragging up
swipes home.

Hover: the phone's pointer follows (absolute reports).
Press: touch down where the phone's pointer already is (absolute report,
button 1).
Held: every 8 ms a relative report moves the touch by exactly what the pointer
moved (sized from the measured table), button 0 on these reports.
Release: absolute report, button 0, where the touch is.
Scroll: iOS ignores the mouse wheel, so wheel scrolling becomes a short swipe
at the pointer: touch down where the pointer is, move the touch by the scrolled
amount, and lift as soon as the wheel stops. Further wheel steps that arrive
before the lift extend the same swipe.

Verified on a touch-event test page (touchlab/): this delivers one touch whose
path matches the request (175 -> 175 pt, flicks 144 -> 141 pt). An absolute
move while pressed makes iOS end the touch after ~90 ms (it reads as a click),
and relative-only presses are ignored.
"""
import threading
import time

from . import touch

MIN_GAP = 1 / 250  # at most this many hover reports per second
SCROLL_IDLE = 0.05  # lift the scroll touch this soon after the wheel stops (s): a short swipe


def clamp(v):
    return min(max(v, 0.0), 1.0)


class Pointer:
    def __init__(self, hid, log=print):
        self.hid = hid  # callable returning a Hid client
        self.log = log
        self.lock = threading.Lock()
        self.pressed = False
        self.at = None  # latest pointer position (unclamped)
        self.sent = None  # last hover position sent = where the phone's pointer is
        self.last_send = 0.0
        self.touch = None  # where the touch is during a drag
        self.carry = [0.0, 0.0]  # phone points still to move
        self.dragged_from = None  # pointer position the carry is measured from
        self.scrolling = False  # the touch is a wheel scroll, not a held button
        self.scrolled_at = 0.0
        self.stopped = False
        self.last_error = None
        threading.Thread(target=self._tick, daemon=True).start()

    def hover(self, fx, fy):
        with self.lock:
            self.at = (fx, fy)
            if self.pressed:
                return
            if time.time() - self.last_send >= MIN_GAP:
                self._send_hover()

    def _send_hover(self):
        # (called with the lock held; also from _tick for a position that was rate-limited)
        pos = (round(clamp(self.at[0]), 4), round(clamp(self.at[1]), 4))
        if pos != self.sent:
            self.hid().cmd(f"a {pos[0]:.5f} {pos[1]:.5f} 0")
            self.sent = pos
            self.last_send = time.time()

    def scroll(self, dx_pt, dy_pt):
        """Scroll by dragging the content dx_pt, dy_pt phone points (positive dy
        moves the content down, like a finger dragging down)."""
        with self.lock:
            if self.pressed and not self.scrolling:
                return  # a held button is already dragging
            if not self.pressed:
                if self.at is None:
                    return
                self._down()
                self.carry = [0.0, 0.0]
                self.scrolling = True
            self.carry[0] += dx_pt
            self.carry[1] += dy_pt
            self.scrolled_at = time.time()

    def _down(self):
        # (lock held) Touch down exactly where the phone's pointer already is (the
        # last hover report), not at the new position: a press that also changes
        # the absolute position makes iOS end the touch after ~90 ms (drags die,
        # clicks get flaky). Any gap is sent as ordinary drag motion.
        fx, fy = self.at
        px, py = self.sent or (clamp(fx), clamp(fy))
        self.hid().cmd(f"a {px:.5f} {py:.5f} 1")
        self.touch = [px, py]
        self.carry = [(clamp(fx) - px) * touch.SCREEN_PT[0], (clamp(fy) - py) * touch.SCREEN_PT[1]]
        self.dragged_from = (fx, fy)
        self.pressed = True

    def _up(self):
        # (lock held) lift where the touch is, so the release carries no position change
        self.pressed = self.scrolling = False
        fx, fy = self.touch
        self.hid().cmd(f"a {fx:.5f} {fy:.5f} 0")
        self.sent = (round(fx, 4), round(fy, 4))

    def press(self, fx, fy):
        with self.lock:
            if self.scrolling:
                self._up()  # a click ends a wheel scroll first
            if self.pressed:
                return
            self.at = (fx, fy)
            self._down()
            self.log("down", f"({self.touch[0]:.3f}, {self.touch[1]:.3f})")

    def release(self):
        """The button went up (a wheel scroll lifts by itself)."""
        with self.lock:
            if not self.pressed or self.scrolling:
                return
            self._up()
            self.log("up", f"({self.touch[0]:.3f}, {self.touch[1]:.3f})")

    def close(self):
        with self.lock:
            if self.pressed:
                self._up()
        self.stopped = True

    def _tick(self):
        # While pressed: every 8 ms, move the touch by exactly what the pointer
        # moved, converted to phone points and then to the report size that moves
        # that far (touch.DRAG_PTS, measured; iOS accelerates by report size and
        # drops reports sent faster than ~8 ms apart). Remainders carry over. One
        # report every tick, even with no motion: steady timing keeps iOS's
        # acceleration predictable. The button stays 0 on these reports: iOS
        # holds the touch from the absolute press, and a button here reads as a
        # second press/release (double tap sounds).
        while not self.stopped:
            time.sleep(touch.TICK)
            try:
                with self.lock:
                    if not self.pressed:
                        if self.at is not None:
                            self._send_hover()
                        continue
                    fx, fy = self.at
                    lx, ly = self.dragged_from
                    self.dragged_from = (fx, fy)
                    if not self.scrolling:  # a wheel scroll ignores the pointer's motion
                        self.carry[0] += (fx - lx) * touch.SCREEN_PT[0]
                        self.carry[1] += (fy - ly) * touch.SCREEN_PT[1]
                    ux, uy = touch.whole_units(*self.carry)  # never more than the pointer moved
                    if self.scrolling and not (ux or uy) and time.time() - self.scrolled_at > SCROLL_IDLE:
                        self._up()
                        continue
                    self.hid().cmd(f"m {ux} {uy} 0 0")
                    if ux or uy:
                        mag = (ux * ux + uy * uy) ** 0.5
                        moved = touch.points_for(mag)
                        self.carry[0] -= moved * ux / mag
                        self.carry[1] -= moved * uy / mag
                        self.touch[0] = clamp(self.touch[0] + moved * ux / mag / touch.SCREEN_PT[0])
                        self.touch[1] = clamp(self.touch[1] + moved * uy / mag / touch.SCREEN_PT[1])
            except Exception as e:
                if str(e) != self.last_error:  # say it once, not every tick
                    self.log("pointer error:", e)
                    self.last_error = str(e)
                time.sleep(0.5)


class Keys:
    """Keyboard state for the phone: modifiers plus up to six held keys (USB HID
    usage codes). Sends a full keyboard report on every change."""

    def __init__(self, hid):
        self.hid = hid
        self.mods = 0
        self.held = []

    def modifier(self, bit, down):
        self.mods = self.mods | bit if down else self.mods & ~bit
        self._send()

    def key(self, usage, down):
        if down and usage not in self.held:
            self.held.append(usage)
        elif not down and usage in self.held:
            self.held.remove(usage)
        else:
            return
        self._send()

    def release_all(self):
        if self.mods or self.held:
            self.mods, self.held = 0, []
            self._send()

    def _send(self):
        self.hid().cmd("k %d %s" % (self.mods, " ".join(str(c) for c in self.held[-6:])))
