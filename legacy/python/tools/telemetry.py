#!/usr/bin/python
"""Record a mouse-on-mirror session for debugging drags and taps.

  telemetry.py start DIR    record until `telemetry.py stop DIR`
  telemetry.py stop DIR

Writes into DIR:
  input.jsonl    raw mouse events from /dev/input (button, motion), wall-clock times
  cursor.jsonl   Hyprland cursor position every ~4 ms, also as phone-screen fractions
  hci.snoop      every Bluetooth packet (btmon; the HID reports we sent)
  video.mp4      the mirror window at 60 fps (wf-recorder)
  meta.json      window geometry and the wall-clock time recording started
"""
import glob
import json
import os
import select
import signal
import struct
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.realpath(__file__))))
from phoneuse import screen as steer  # noqa: E402
from phoneuse.screen import hypr  # noqa: E402

EVENT = struct.Struct("llHHi")


def record(d):
    os.makedirs(d, exist_ok=True)
    stopfile = os.path.join(d, "STOP")
    if os.path.exists(stopfile):
        os.unlink(stopfile)
    client = steer.mirror_client()
    (wx, wy), (ww, wh) = client["at"], client["size"]
    pic = steer.picture_rect(client["at"], client["size"])
    geom = f"{wx},{wy} {ww}x{wh}"
    btmon = subprocess.Popen(["sudo", "btmon", "-w", os.path.join(d, "hci.snoop")],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t_video = time.time()
    video = subprocess.Popen(["wf-recorder", "-g", geom, "-r", "60", "-c", "libx264",
                              "-p", "preset=ultrafast", "-p", "crf=18", "-y", "-f", os.path.join(d, "video.mp4")],
                             stdout=subprocess.DEVNULL, stderr=open(os.path.join(d, "wf-recorder.log"), "w"))
    json.dump({"started": t_video, "window_at": [wx, wy], "window_size": [ww, wh],
               "picture": pic, "video_geom": geom}, open(os.path.join(d, "meta.json"), "w"), indent=1)
    stop = threading.Event()

    def mouse():
        fds = {os.open(p, os.O_RDONLY | os.O_NONBLOCK): os.path.basename(p) for p in glob.glob("/dev/input/by-id/*event-mouse")}
        with open(os.path.join(d, "input.jsonl"), "w") as f:
            while not stop.is_set():
                r, _, _ = select.select(list(fds), [], [], 0.2)
                for fd in r:
                    data = os.read(fd, EVENT.size * 64)
                    for i in range(0, len(data) - EVENT.size + 1, EVENT.size):
                        sec, usec, ty, code, val = EVENT.unpack_from(data, i)
                        if ty in (1, 2):
                            f.write(json.dumps({"t": sec + usec / 1e6, "dev": fds[fd][4:24], "type": ty, "code": code, "value": val}) + "\n")

    def cursor():
        x, y, w, h = pic
        with open(os.path.join(d, "cursor.jsonl"), "w") as f:
            while not stop.is_set():
                c = hypr("j/cursorpos")
                f.write(json.dumps({"t": time.time(), "x": c["x"], "y": c["y"],
                                    "fx": round((c["x"] - x) / w, 5), "fy": round((c["y"] - y) / h, 5)}) + "\n")
                time.sleep(0.004)

    threads = [threading.Thread(target=mouse), threading.Thread(target=cursor)]
    for t in threads:
        t.start()
    print(f"recording into {d}", flush=True)
    while not os.path.exists(stopfile):
        time.sleep(0.2)
    stop.set()
    for t in threads:
        t.join()
    video.send_signal(signal.SIGINT)
    video.wait(10)
    subprocess.run(["sudo", "pkill", "-INT", "-x", "btmon"])
    btmon.wait(5)
    print("stopped", flush=True)


if __name__ == "__main__":
    cmd, d = sys.argv[1], sys.argv[2]
    if cmd == "start":
        record(d)
    elif cmd == "stop":
        open(os.path.join(d, "STOP"), "w").close()
