"""Client for the phoneuse HID daemon's command socket (see hid_daemon.py)."""
import socket
import threading

from .config import HID_SOCKET


class Hid:
    """One connection to the daemon. Thread-safe: commands are serialized."""

    def __init__(self, path=HID_SOCKET):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.f = self.sock.makefile()
        self.lock = threading.Lock()

    def cmd(self, line):
        with self.lock:
            self.sock.sendall((line + "\n").encode())
            reply = self.f.readline().strip()
        if not reply.startswith("ok"):
            raise RuntimeError(reply)
        return reply
