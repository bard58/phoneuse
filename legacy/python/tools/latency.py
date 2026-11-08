#!/usr/bin/python
"""Measure where the mirror's delay goes, using pointer jumps.

For each jump of the iPhone pointer (one absolute HID report) it records:
  t_send   the report leaving hidd
  t_net    the first AirPlay video packet after it (tcpdump on TCP 7100; iOS only
           sends frames when the screen changes, so this is the frame that shows it)
  t_shown  the dot appearing in the mirror window (fast raw grabs of a small patch)
Phone side = t_net - t_send (Bluetooth + iOS capture/encode + Wi-Fi).
PC side    = t_shown - t_net (decode + draw + compositor + grab).
Keep the mouse off the mirror while it runs. Needs sudo for tcpdump.
"""
import statistics as st
import os
import subprocess
import sys
import time

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.realpath(__file__))))
from phoneuse import screen as steer  # noqa: E402
from phoneuse.hidclient import Hid  # noqa: E402

N = int(sys.argv[1]) if len(sys.argv) > 1 else 12
SPOTS = [(0.3, 0.62), (0.7, 0.62)]


def patch_rect(fx, fy, half=0.06):
    x, y, w, h = steer.picture_rect(*steer.mirror_window())
    px, py = x + fx * w, y + fy * h
    r = half * w
    return round(px - r), round(py - r), round(2 * r), round(2 * r)


def grab_patch(rect):
    x, y, w, h = rect
    ppm = subprocess.run(["grim", "-t", "ppm", "-g", f"{x},{y} {w}x{h}", "-"], capture_output=True).stdout
    # P6\n<w> <h>\n255\n<data>
    parts = ppm.split(b"\n", 3)
    pw, ph = map(int, parts[1].split())
    return np.frombuffer(parts[3], np.uint8)[: pw * ph * 3].reshape(ph, pw, 3).astype(np.int16)


def main():
    hid = Hid()
    tcpdump = subprocess.Popen(["sudo", "tcpdump", "-l", "-tt", "-n", "-i", "any", "tcp port 7100 and greater 300"],
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    time.sleep(1.5)
    rows = []
    for i in range(N):
        tx, ty = SPOTS[i % 2]
        ox, oy = SPOTS[(i + 1) % 2]
        hid.cmd(f"a {ox} {oy} 0")  # park on the other spot
        time.sleep(1.0)
        rect = patch_rect(tx, ty)
        base = grab_patch(rect)
        t_send = time.time()
        hid.cmd(f"a {tx} {ty} 0")
        t_shown = None
        while time.time() - t_send < 2.0:
            img = grab_patch(rect)
            if np.abs(img - base).mean() > 6:
                t_shown = time.time()
                break
        rows.append((t_send, t_shown))
        time.sleep(0.3)
    time.sleep(0.5)
    tcpdump.terminate()
    pkts = []
    for line in tcpdump.stdout.read().splitlines():
        try:
            pkts.append(float(line.split()[0]))
        except (ValueError, IndexError):
            pass
    phone, pc, total = [], [], []
    for t_send, t_shown in rows:
        t_net = next((t for t in pkts if t > t_send), None)
        if t_shown and t_net and t_net < t_shown:
            phone.append((t_net - t_send) * 1000)
            pc.append((t_shown - t_net) * 1000)
            total.append((t_shown - t_send) * 1000)
    if not total:
        print("no complete samples (is the mirror up and the mouse off it?)")
        return
    f = lambda v: f"median {st.median(v):5.0f} ms  (min {min(v):4.0f}, max {max(v):4.0f})"
    print(f"samples {len(total)}/{N}; video packets seen {len(pkts)}")
    print(f"  phone side (BT + iOS encode + Wi-Fi): {f(phone)}")
    print(f"  PC side (decode + draw + grab):       {f(pc)}")
    print(f"  total:                                {f(total)}")


if __name__ == "__main__":
    main()
