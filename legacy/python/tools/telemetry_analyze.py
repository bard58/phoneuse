#!/usr/bin/python
"""Analyze a telemetry.py session: line up mouse input, HID reports and the orange
pointer seen in the mirror video.

  telemetry_analyze.py DIR      writes DIR/reports.jsonl, DIR/circle.jsonl, prints a summary
"""
import datetime
import json
import os
import re
import subprocess
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.realpath(__file__))))
from phoneuse import screen as steer  # noqa: E402


def s8(v):
    return v - 256 if v > 127 else v


def hid_reports(d):
    out = subprocess.run(["sudo", "btmon", "-r", os.path.join(d, "hci.snoop"), "-T"],
                         capture_output=True, text=True).stdout.splitlines()
    reps, t = [], None
    for line in out:
        m = re.match(r"^< ACL: .*?(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d+)\s*$", line)
        if m:
            t = datetime.datetime.strptime(m.group(1), "%Y-%m-%d %H:%M:%S.%f").timestamp()
            continue
        h = re.match(r"^\s+a1 (0[23])((?: [0-9a-f]{2})+)", line)
        if h and t is not None:
            b = [int(x, 16) for x in h.group(2).split()]
            if h.group(1) == "03":
                reps.append({"t": t, "kind": "abs", "btn": b[0], "fx": (b[1] | b[2] << 8) / 32767, "fy": (b[3] | b[4] << 8) / 32767})
            else:
                reps.append({"t": t, "kind": "rel", "btn": b[0], "dx": s8(b[1]), "dy": s8(b[2])})
            t = None
    return reps


def circle_track(d, meta, step=1):
    """Orange ring position per video frame, as phone-screen fractions; t is video time."""
    vx, vy = [int(v) for v in meta["video_geom"].split()[0].split(",")]
    px, py, pw, ph = meta["picture"]
    probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v", "-show_entries", "stream=width,height",
                            "-of", "csv=p=0", os.path.join(d, "video.mp4")], capture_output=True, text=True).stdout.strip()
    W, H = map(int, probe.split(","))
    vw = int(meta["video_geom"].split()[1].split("x")[0])
    scale = W / vw  # physical px per logical px
    # crop the picture area and halve it for speed
    cx, cy, cw, ch = [round(v * scale) for v in (px - vx, py - vy, pw, ph)]
    ow, oh = cw // 2, ch // 2
    ff = subprocess.Popen(["ffmpeg", "-v", "error", "-i", os.path.join(d, "video.mp4"),
                           "-vf", f"crop={cw}:{ch}:{cx}:{cy},scale={ow}:{oh}", "-f", "rawvideo", "-pix_fmt", "rgb24",
                           "-"], stdout=subprocess.PIPE)
    pts = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v", "-show_entries", "frame=pts_time",
                          "-of", "csv=p=0", os.path.join(d, "video.mp4")], capture_output=True, text=True).stdout.replace(",", " ").split()
    track = []
    n = 0
    while True:
        buf = ff.stdout.read(ow * oh * 3)
        if len(buf) < ow * oh * 3:
            break
        if n % step == 0:
            frame = np.frombuffer(buf, np.uint8).reshape(oh, ow, 3)
            p = steer.find_orange(frame)
            track.append({"vt": float(pts[n]) if n < len(pts) else None, "fx": p[0] if p else None, "fy": p[1] if p else None})
        n += 1
    return track


def load_jsonl(p):
    return [json.loads(l) for l in open(p)]


def main(d):
    meta = json.load(open(os.path.join(d, "meta.json")))
    reps = hid_reports(d)
    json.dump(reps, open(os.path.join(d, "reports.json"), "w"))
    print(f"HID reports: {len(reps)} ({sum(r['kind'] == 'abs' for r in reps)} absolute, {sum(r['kind'] == 'rel' for r in reps)} relative)")
    cpath = os.path.join(d, "circle.json")
    if not os.path.exists(cpath):
        track = circle_track(d, meta)
        json.dump(track, open(cpath, "w"))
    track = json.load(open(cpath))
    print(f"video frames analyzed: {len(track)}, circle found in {sum(t['fx'] is not None for t in track)}")


if __name__ == "__main__":
    main(sys.argv[1])


def holds_report(d):
    meta = json.load(open(os.path.join(d, "meta.json")))
    inp = load_jsonl(os.path.join(d, "input.jsonl"))
    cur = load_jsonl(os.path.join(d, "cursor.jsonl"))
    reps = json.load(open(os.path.join(d, "reports.json")))
    circ = [c for c in json.load(open(os.path.join(d, "circle.json"))) if c["fx"] is not None and c["vt"] is not None]
    PT = (402, 874)
    ct = np.array([c["t"] for c in cur]); cfx = np.array([c["fx"] for c in cur]); cfy = np.array([c["fy"] for c in cur])
    def cursor_at(t):
        i = np.clip(np.searchsorted(ct, t), 1, len(ct) - 1)
        return cfx[i], cfy[i]
    # presses from raw input
    presses, down = [], None
    for e in inp:
        if e["type"] == 1 and e["code"] == 272:
            if e["value"] == 1: down = e["t"]
            elif e["value"] == 0 and down is not None: presses.append((down, e["t"])); down = None
    def pressed(t):
        return any(a - 0.05 <= t <= b + 0.3 for a, b in presses)
    # align video time to wall clock using hover-only frames
    hv = [c for c in circ if not pressed(meta["started"] + c["vt"])]
    best = None
    for s in np.arange(-1.0, 1.0, 0.01):
        errs = []
        for c in hv[::3]:
            fx, fy = cursor_at(meta["started"] + c["vt"] + s)
            errs.append(abs(fx - c["fx"]) * PT[0] + abs(fy - c["fy"]) * PT[1])
        m = float(np.median(errs))
        if best is None or m < best[1]: best = (s, m)
    s = best[0]
    print(f"video aligned: cursor time = video time + {s:+.2f} s (hover match median error {best[1]:.1f} pt)")
    lag = 0.10  # measured earlier: report -> visible in mirror
    vt_wall = np.array([meta["started"] + c["vt"] + s + lag for c in circ])  # wall time of the HID report a frame reflects
    cx = np.array([c["fx"] for c in circ]); cy = np.array([c["fy"] for c in circ])
    print(f"\n{'#':>2} {'held':>5}  {'cursor moved':>14}  {'abs press/rel':>13}  {'rel sent':>12}  {'circle moved':>14}  {'circle jitter':>13}")
    for k, (a, b) in enumerate(presses):
        # cursor during hold
        m = (ct >= a) & (ct <= b)
        if m.sum() < 2:
            continue
        dxp = (cfx[m][-1] - cfx[m][0]) * PT[0]; dyp = (cfy[m][-1] - cfy[m][0]) * PT[1]
        # reports during hold
        hr = [r for r in reps if a - 0.02 <= r["t"] <= b + 0.05]
        presses_sent = sum(1 for r in hr if r["kind"] == "abs" and r["btn"] == 1)
        rel = [r for r in hr if r["kind"] == "rel"]
        rux = sum(r["dx"] for r in rel); ruy = sum(r["dy"] for r in rel)
        # circle during hold
        cm = (vt_wall >= a) & (vt_wall <= b)
        if cm.sum() >= 2:
            px_, py_ = cx[cm] * PT[0], cy[cm] * PT[1]
            net = ((px_[-1] - px_[0]) ** 2 + (py_[-1] - py_[0]) ** 2) ** 0.5
            path = float(np.sum(np.hypot(np.diff(px_), np.diff(py_))))
            circ_s = f"{px_[-1]-px_[0]:+5.0f},{py_[-1]-py_[0]:+5.0f}"
            jit = f"path/net {path/max(net,1):4.1f}"
        else:
            circ_s, jit = "   (not seen)", ""
        print(f"{k:>2} {b-a:5.2f}  {dxp:+6.0f},{dyp:+6.0f}  {presses_sent:>3}/{len(rel):<4}      {rux:+5d},{ruy:+5d}  {circ_s:>14}  {jit:>13}")


if __name__ == "__main__" and len(sys.argv) > 2 and sys.argv[2] == "holds":
    holds_report(sys.argv[1])
