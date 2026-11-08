#!/usr/bin/python
"""Bluetooth HID mouse + keyboard for an iPhone, driven over a Unix socket.

Run as root. bluetoothd must be started with --noplugin=input so this process
can own the HID PSMs (17 control, 19 interrupt). See install.sh.

Commands on /run/phoneuse-hid.sock, one per line, reply "ok ..." or "err ...":
  move DX DY        relative pointer move (split into small steps)
  click [left|right|middle]
  down [btn] / up   hold / release a button (for drags)
  scroll N          wheel; positive scrolls up
  key NAME [MOD..]  e.g. "key h gui" (Cmd+H = home), "key enter", "key tab shift"
  type TEXT         US-layout text
  corner            slam pointer to the top-left corner
  connect           page the paired iPhone now (the daemon also reconnects by itself)
  pair [SECONDS]    discoverable/pairable for SECONDS (0 = always, the startup default)
  m DX DY [W [BTN]] raw mouse report (live input)
  k MODS [CODE..]   raw keyboard report: held modifiers + up to 6 HID usage codes
  status
"""
import os
import socket
import struct
import threading
import time

import dbus
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

HERE = os.path.dirname(os.path.abspath(__file__))
from .config import HID_SOCKET as SOCK_PATH, ALLOWED_FILE as ALLOW_FILE
DEVICE_NAME = os.environ.get("PHONEUSE_NAME", "PhoneUse")  # what the phone sees in Bluetooth settings
PSM_CTRL, PSM_INTR = 0x11, 0x13
HID_UUID = "00001124-0000-1000-8000-00805f9b34fb"
PROFILE_PATH = "/io/phoneuse/hid/profile"
AGENT_PATH = "/io/phoneuse/hid/agent"

# Report 1: boot-style keyboard (mods, reserved, 6 keys). Report 2: mouse (buttons, x, y, wheel).
REPORT_MAP = bytes.fromhex(
    "05010906a1018501050719e029e71500250175019508810295017508810395067508150025650507190029658100c0"
    +"05010902a10185020901a1000509190129031500250195037501810295017505810305010930093109381581257f750895038106c0c0"
    # Report 3: absolute pointer (buttons, X and Y 0..32767). Experimental: tests
    # whether iOS will place the AssistiveTouch pointer at exact coordinates.
    + "05010902a10185030901a1000509190129031500250195037501810295017505810305010930093115002"
    "6ff7f751095028102c0c0"
    # Report 4: consumer control, one bit: Eject. On iOS it shows or hides the
    # on-screen keyboard while a hardware keyboard (this one) is connected.
    + "050c0901a10185041500250175019501" "09b88102" "95078103c0"
)


def log(*a):
    print(time.strftime("%H:%M:%S"), *a, flush=True)


# ---------- key tables (US layout) ----------
MODS = {"ctrl": 0x01, "shift": 0x02, "alt": 0x04, "gui": 0x08, "cmd": 0x08,
        "rctrl": 0x10, "rshift": 0x20, "ralt": 0x40, "rgui": 0x80}
NAMED = {"enter": 40, "return": 40, "esc": 41, "escape": 41, "backspace": 42, "tab": 43,
         "space": 44, "right": 79, "left": 80, "down": 81, "up": 82, "delete": 76,
         "home": 74, "end": 77, "pageup": 75, "pagedown": 78}
CHARS = {}
for i, c in enumerate("abcdefghijklmnopqrstuvwxyz"):
    CHARS[c] = (4 + i, 0)
    CHARS[c.upper()] = (4 + i, 0x02)
for i, c in enumerate("1234567890"):
    CHARS[c] = (30 + i, 0)
for i, c in enumerate("!@#$%^&*()"):
    CHARS[c] = (30 + i, 0x02)
for plain, shifted, code in [("-", "_", 45), ("=", "+", 46), ("[", "{", 47), ("]", "}", 48),
                             ("\\", "|", 49), (";", ":", 51), ("'", '"', 52), ("`", "~", 53),
                             (",", "<", 54), (".", ">", 55), ("/", "?", 56)]:
    CHARS[plain] = (code, 0)
    CHARS[shifted] = (code, 0x02)
CHARS[" "] = (44, 0)
CHARS["\n"] = (40, 0)
CHARS["\t"] = (43, 0)
BUTTONS = {"left": 1, "right": 2, "middle": 4}


class HID:
    def __init__(self):
        self.ctrl = None
        self.intr = None
        self.peer = None
        self.buttons = 0
        self.lock = threading.Lock()

    @property
    def connected(self):
        return self.intr is not None

    def send(self, report_id, payload):
        with self.lock:
            if not self.intr:
                reconnect_now.set()
                raise RuntimeError("no iPhone connected, reconnecting")
            try:
                self.intr.send(bytes([0xA1, report_id]) + bytes(payload))
            except OSError as e:
                self.drop(f"send failed: {e}")
                raise RuntimeError("connection lost")

    def drop(self, why):
        log("disconnect:", why)
        for s in (self.intr, self.ctrl):
            try:
                s and s.close()
            except OSError:
                pass
        self.ctrl = self.intr = None
        reconnect_now.set()

    # mouse
    def mouse(self, dx=0, dy=0, wheel=0):
        b = lambda v: v & 0xFF
        self.send(2, [self.buttons, b(dx), b(dy), b(wheel)])

    def move(self, dx, dy, step=8, delay=0.008):
        # Small, evenly paced steps keep iOS pointer acceleration predictable.
        while dx or dy:
            sx = max(-step, min(step, dx))
            sy = max(-step, min(step, dy))
            self.mouse(sx, sy)
            dx -= sx
            dy -= sy
            time.sleep(delay)

    def corner(self):
        for _ in range(60):
            self.mouse(-127, -127)
            time.sleep(0.008)

    def press(self, btn):
        self.buttons |= BUTTONS[btn]
        self.mouse()

    def release(self):
        self.buttons = 0
        self.mouse()

    def click(self, btn="left"):
        self.press(btn)
        time.sleep(0.05)
        self.release()

    def scroll(self, n):
        sign = 1 if n > 0 else -1
        for _ in range(abs(n)):
            self.mouse(wheel=sign)
            time.sleep(0.02)

    # keyboard
    def eject(self):
        """Press and release Eject: toggles the iPhone's on-screen keyboard."""
        self.send(4, [1])
        time.sleep(0.05)
        self.send(4, [0])

    def key(self, code, mods=0):
        self.send(1, [mods, 0, code, 0, 0, 0, 0, 0])
        time.sleep(0.012)
        self.send(1, [0] * 8)
        time.sleep(0.012)

    def type(self, text):
        for ch in text:
            if ch not in CHARS:
                raise ValueError(f"cannot type {ch!r}")
            self.key(*CHARS[ch])


hid = HID()


# ---------- link tuning ----------
# The iPhone is the master of the link and may poll us only every ~50 ms, which
# caps the pointer at ~20 updates/s (measured 2026-11-06: acks 51 ms apart).
# A QoS Setup asking for 8 ms latency brings that to ~2 ms, as real mice do.
HCIGETCONNINFO = 0x800448D5


def acl_handle(addr):
    import fcntl
    s = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW, socket.BTPROTO_HCI)
    try:
        s.bind((0,))
        ba = bytes(int(x, 16) for x in reversed(addr.split(":")))
        buf = bytearray(ba + bytes([1]) + bytes(24))  # hci_conn_info_req, type ACL
        fcntl.ioctl(s.fileno(), HCIGETCONNINFO, buf, True)
        return struct.unpack_from("<H", buf, 8)[0]
    finally:
        s.close()


def tune_link(addr):
    try:
        h = acl_handle(addr)
        s = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_RAW, socket.BTPROTO_HCI)
        s.bind((0,))

        def cmd(ogf, ocf, p):
            s.send(bytes([1]) + struct.pack("<HB", (ogf << 10) | ocf, len(p)) + p)
        cmd(0x02, 0x000D, struct.pack("<HH", h, 0x0001))  # link policy: role switch only, no sniff
        time.sleep(0.02)
        # QoS Setup: guaranteed, 800 B/s, latency 8000 us
        cmd(0x02, 0x0007, struct.pack("<HBBIIII", h, 0, 0x02, 800, 0, 8000, 0xFFFFFFFF))
        s.close()
        return h
    except OSError as e:
        log("tune_link failed:", e)
        return None


def tune_loop():
    while True:
        time.sleep(20)
        if hid.connected and hid.peer:
            tune_link(hid.peer)


# ---------- L2CAP ----------
def handle_ctrl(sock):
    # Answer HID control-channel requests (SET_PROTOCOL, SET_IDLE, ...) with HANDSHAKE OK.
    while True:
        try:
            data = sock.recv(64)
        except OSError:
            break
        if not data:
            break
        t = data[0] >> 4
        if t == 0x0:  # HANDSHAKE from host: ignore
            continue
        if t == 0x1 and data[0] & 0x0F in (0x3, 0x4, 0x5):  # HID_CONTROL suspend/exit/unplug
            log("control op", hex(data[0]))
            continue
        try:
            if t == 0x4:  # GET_REPORT -> empty report of the asked id
                rid = data[1] if len(data) > 1 else 1
                sock.send(bytes([0xA1, rid]) + bytes(8 if rid == 1 else 4))
            elif t == 0x6:  # GET_PROTOCOL -> report protocol
                sock.send(b"\xa0\x01")
            else:
                sock.send(b"\x00")
        except OSError:
            break
    if hid.ctrl is sock:
        hid.drop("control channel closed")


def listen(psm):
    s = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_SEQPACKET, socket.BTPROTO_L2CAP)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((socket.BDADDR_ANY, psm))
    s.listen(1)
    return s


def accept_loop():
    sc, si = listen(PSM_CTRL), listen(PSM_INTR)
    log("listening on PSM 17/19")
    while True:
        c, addr = sc.accept()
        if not allowed(addr[0]):
            log("rejected HID connection from", addr[0], "(not in", ALLOW_FILE + ")")
            c.close()
            continue
        log("control from", addr[0])
        i, _ = si.accept()
        log("interrupt from", addr[0])
        if hid.connected:
            hid.drop("replaced by new connection")
        hid.ctrl, hid.intr, hid.peer = c, i, addr[0]
        threading.Thread(target=handle_ctrl, args=(c,), daemon=True).start()
        log("link tuned, handle", tune_link(addr[0]))


connect_lock = threading.Lock()
reconnect_now = threading.Event()


def connect_out():
    """Page each paired iPhone once and open the HID channels."""
    with connect_lock:
        if hid.connected:
            return "already connected"
        errors = []
        for addr in paired_devices():
            socks = []
            try:
                for psm in (PSM_CTRL, PSM_INTR):
                    s = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_SEQPACKET, socket.BTPROTO_L2CAP)
                    socks.append(s)
                    s.settimeout(8)
                    s.connect((addr, psm))
                    s.settimeout(None)
            except OSError as e:
                for s in socks:
                    s.close()
                errors.append(f"{addr}: {e}")
                continue
            if hid.connected:  # the iPhone connected in by itself meanwhile
                for s in socks:
                    s.close()
                return "already connected"
            hid.ctrl, hid.intr = socks
            hid.peer = addr
            log("connected out to", addr, "- link tuned, handle", tune_link(addr))
            threading.Thread(target=handle_ctrl, args=(hid.ctrl,), daemon=True).start()
            return f"connected to {addr}"
        raise RuntimeError("; ".join(errors) or "no paired iPhone")


def reconnect_loop():
    # While disconnected, page the iPhone with a growing pause between tries
    # (5 s up to 30 s). Paging without pauses keeps the radio busy and makes it
    # miss the iPhone's own connection attempts. A disconnect, a command sent
    # while disconnected, or the iPhone coming into range (BlueZ sees it
    # connect) cuts the pause short.
    pause = 0
    while True:
        reconnect_now.wait(30 if hid.connected else pause)
        woke = reconnect_now.is_set()
        reconnect_now.clear()
        if hid.connected:
            pause = 0
            continue
        if woke:
            time.sleep(3)  # give the iPhone a moment to reconnect by itself
            if hid.connected:
                continue
        try:
            log("reconnect:", connect_out())
            pause = 0
        except Exception as e:
            pause = min(30, max(5, pause * 2))
            log(f"reconnect failed ({e}); next try in {pause}s")


def on_device_changed(iface, changed, invalidated, path=None):
    if iface == "org.bluez.Device1" and changed.get("Connected") and not hid.connected:
        reconnect_now.set()


# ---------- D-Bus: profile, agent, adapter ----------
bus = None


def adapter():
    return dbus.Interface(bus.get_object("org.bluez", "/org/bluez/hci0"), "org.freedesktop.DBus.Properties")


def paired_devices():
    om = dbus.Interface(bus.get_object("org.bluez", "/"), "org.freedesktop.DBus.ObjectManager")
    out = []
    for path, ifaces in om.GetManagedObjects().items():
        d = ifaces.get("org.bluez.Device1")
        if d and d.get("Paired"):
            out.append(str(d["Address"]))
    return out


class Profile(dbus.service.Object):
    @dbus.service.method("org.bluez.Profile1", in_signature="", out_signature="")
    def Release(self):
        pass

    @dbus.service.method("org.bluez.Profile1", in_signature="oha{sv}", out_signature="")
    def NewConnection(self, path, fd, props):
        os.close(fd.take())  # we own the PSMs directly

    @dbus.service.method("org.bluez.Profile1", in_signature="o", out_signature="")
    def RequestDisconnection(self, path):
        pass


class Agent(dbus.service.Object):
    # NoInputNoOutput: accept every pairing ("Just Works"). Pairing is only possible while
    # the adapter is discoverable, which the "pair" command opens for a limited time.
    @dbus.service.method("org.bluez.Agent1", in_signature="", out_signature="")
    def Release(self):
        pass

    @dbus.service.method("org.bluez.Agent1", in_signature="os", out_signature="")
    def AuthorizeService(self, device, uuid):
        addr = str(device).rsplit("dev_", 1)[-1].replace("_", ":")
        if not allowed(addr):
            log("rejected service", uuid, "for", addr)
            raise dbus.exceptions.DBusException("not allowed", name="org.bluez.Error.Rejected")

    @dbus.service.method("org.bluez.Agent1", in_signature="ou", out_signature="")
    def RequestConfirmation(self, device, passkey):
        log("pairing confirm", device, passkey)

    @dbus.service.method("org.bluez.Agent1", in_signature="o", out_signature="")
    def RequestAuthorization(self, device):
        pass

    @dbus.service.method("org.bluez.Agent1", in_signature="o", out_signature="s")
    def RequestPinCode(self, device):
        return "0000"

    @dbus.service.method("org.bluez.Agent1", in_signature="o", out_signature="u")
    def RequestPasskey(self, device):
        return dbus.UInt32(0)

    @dbus.service.method("org.bluez.Agent1", in_signature="ouq", out_signature="")
    def DisplayPasskey(self, device, passkey, entered):
        log("passkey", passkey)

    @dbus.service.method("org.bluez.Agent1", in_signature="os", out_signature="")
    def DisplayPinCode(self, device, pin):
        log("pin", pin)

    @dbus.service.method("org.bluez.Agent1", in_signature="", out_signature="")
    def Cancel(self):
        pass




def allowed(addr):
    """Only listed devices may use the HID channels. Trust on first use: while the
    list is empty, the first device to connect is added to it."""
    try:
        allow = {l.split("#")[0].strip().upper() for l in open(ALLOW_FILE)} - {""}
    except FileNotFoundError:
        allow = set()
    if not allow:
        os.makedirs(os.path.dirname(ALLOW_FILE), exist_ok=True)
        with open(ALLOW_FILE, "a") as f:
            f.write(f"{addr.upper()}  # first device to pair\n")
        log("allow list was empty; trusting", addr)
        return True
    return addr.upper() in allow


def open_pairing(seconds):
    a = adapter()
    a.Set("org.bluez.Adapter1", "PairableTimeout", dbus.UInt32(seconds))
    a.Set("org.bluez.Adapter1", "DiscoverableTimeout", dbus.UInt32(seconds))
    a.Set("org.bluez.Adapter1", "Pairable", True)
    a.Set("org.bluez.Adapter1", "Discoverable", True)


def setup_dbus():
    global bus
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SystemBus()
    record = (open(os.path.join(HERE, "sdp_record.xml")).read()
              .replace("@REPORT_MAP@", REPORT_MAP.hex()).replace("@NAME@", DEVICE_NAME))
    Profile(bus, PROFILE_PATH)
    pm = dbus.Interface(bus.get_object("org.bluez", "/org/bluez"), "org.bluez.ProfileManager1")
    pm.RegisterProfile(PROFILE_PATH, HID_UUID, {
        "ServiceRecord": record, "Role": "server",
        "RequireAuthentication": False, "RequireAuthorization": False})
    Agent(bus, AGENT_PATH)
    am = dbus.Interface(bus.get_object("org.bluez", "/org/bluez"), "org.bluez.AgentManager1")
    am.RegisterAgent(AGENT_PATH, "NoInputNoOutput")
    am.RequestDefaultAgent(AGENT_PATH)
    a = adapter()
    a.Set("org.bluez.Adapter1", "Powered", True)
    a.Set("org.bluez.Adapter1", "Alias", DEVICE_NAME)
    bus.add_signal_receiver(on_device_changed, "PropertiesChanged",
                            "org.freedesktop.DBus.Properties", "org.bluez", path_keyword="path")
    log("profile + agent registered")
    # Always discoverable and pairable, so the iPhone can (re)pair any time.
    # The allow list keeps other devices off the HID channels.
    open_pairing(0)


# ---------- command socket ----------
def run_command(line):
    parts = line.strip().split(" ", 1)
    cmd, rest = parts[0].lower(), (parts[1] if len(parts) > 1 else "")
    args = rest.split()
    if cmd == "status":
        return f"ok connected={hid.connected} peer={hid.peer} paired={paired_devices()}"
    if cmd == "connect":
        return "ok " + connect_out()
    if cmd == "eject":
        hid.eject()
        return "ok"
    if cmd == "pair":
        secs = int(args[0]) if args else 120
        GLib.idle_add(open_pairing, secs)
        return f"ok discoverable for {secs}s as '{DEVICE_NAME}'"
    if cmd == "a":  # absolute pointer: a FX FY [BUTTONS], FX/FY are 0..1 screen fractions
        x, y = (max(0, min(32767, round(float(v) * 32767))) for v in args[:2])
        btn = int(args[2]) & 0x07 if len(args) > 2 else 0
        hid.send(3, [btn, x & 0xFF, x >> 8, y & 0xFF, y >> 8])
        return "ok"
    if cmd == "m":  # raw mouse report: m DX DY [WHEEL [BUTTONS]], for live input
        dx, dy = (max(-127, min(127, int(v))) for v in args[:2])
        wheel = max(-127, min(127, int(args[2]))) if len(args) > 2 else 0
        if len(args) > 3:
            hid.buttons = int(args[3]) & 0x07
        hid.mouse(dx, dy, wheel)
        return "ok"
    if cmd == "k":  # raw keyboard report: k MODS [CODE...], up to 6 held keys
        codes = [int(c) & 0xFF for c in args[1:7]]
        hid.send(1, [int(args[0]) & 0xFF, 0] + codes + [0] * (6 - len(codes)))
        return "ok"
    if cmd == "move":
        hid.move(int(args[0]), int(args[1]))
    elif cmd == "corner":
        hid.corner()
    elif cmd == "click":
        hid.click(args[0] if args else "left")
    elif cmd == "down":
        hid.press(args[0] if args else "left")
    elif cmd == "up":
        hid.release()
    elif cmd == "scroll":
        hid.scroll(int(args[0]))
    elif cmd == "key":
        name, mods = args[0].lower(), 0
        for m in args[1:]:
            mods |= MODS[m.lower()]
        if name in NAMED:
            code = NAMED[name]
        else:
            code, shift = CHARS[name]
            mods |= shift
        hid.key(code, mods)
    elif cmd == "type":
        hid.type(rest)
    else:
        return f"err unknown command {cmd!r}"
    return "ok"


def serve_client(conn):
    with conn, conn.makefile("rw") as f:
        for line in f:
            if not line.strip():
                continue
            try:
                reply = run_command(line)
            except Exception as e:
                reply = f"err {e}"
            f.write(reply + "\n")
            f.flush()


def command_server():
    try:
        os.unlink(SOCK_PATH)
    except FileNotFoundError:
        pass
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.bind(SOCK_PATH)
    os.chmod(SOCK_PATH, 0o660)
    gid = int(os.environ.get("HID_GID", "0"))
    os.chown(SOCK_PATH, 0, gid)
    s.listen(4)
    log("commands on", SOCK_PATH)
    while True:
        conn, _ = s.accept()
        threading.Thread(target=serve_client, args=(conn,), daemon=True).start()


def main():
    setup_dbus()
    threading.Thread(target=accept_loop, daemon=True).start()
    threading.Thread(target=command_server, daemon=True).start()
    threading.Thread(target=tune_loop, daemon=True).start()

    threading.Thread(target=reconnect_loop, daemon=True).start()
    reconnect_now.set()
    GLib.MainLoop().run()


if __name__ == "__main__":
    main()
