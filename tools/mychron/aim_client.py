#!/usr/bin/env python3
"""MyChron6 STCP protocol client — exercises the operations documented in
docs/mychron-protocol.md against the live device, verifies framing claims,
and collects artifacts for open-question analysis.

Usage: aim_client.py <mode> [args]
  probe                 hello, time sync, info blob, users, file list,
                        catalog/tree/schema, UDP descriptor
  splash                file-stat probes for splash.bmp path variants
  live <seconds>        start stream, poll objects 0x03/0x53, stop, post-stop probe
  download <name>       open + chunked download of a device path
  objprobe <id> <kind> [path]  single NC on a dedicated connection
"""
import json
import os
import socket
import struct
import sys
import time
import zlib
from datetime import datetime, timezone

HOST = "11.0.0.1"
OUT = os.path.expanduser("~/mychron_probe")


def frame(tag: bytes, payload: bytes) -> bytes:
    return (b"<hST" + tag + struct.pack("<I", len(payload)) + b"\x00>"
            + payload
            + b"<ST" + tag + struct.pack("<H", sum(payload) & 0xFFFF) + b">")


class Reader:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""
        self.frames = 0

    def _fill(self, timeout):
        self.sock.settimeout(timeout)
        chunk = self.sock.recv(65536)
        if not chunk:
            raise EOFError("device closed connection")
        self.buf += chunk

    def read_frame(self, timeout=6.0):
        while True:
            j = self.buf.find(b"<hST")
            if j >= 0 and len(self.buf) >= j + 12:
                tag = self.buf[j + 4:j + 6]
                if tag in (b"CP", b"NC"):
                    n = struct.unpack("<I", self.buf[j + 6:j + 10])[0]
                    if self.buf[j + 10] == 0 and self.buf[j + 11] == 0x3E:
                        need = j + 12 + n + 8
                        if len(self.buf) >= need:
                            payload = self.buf[j + 12:j + 12 + n]
                            tr = self.buf[j + 12 + n:need]
                            assert tr[:5] == b"<ST" + tag and tr[7:8] == b">", tr
                            s = struct.unpack("<H", tr[5:7])[0]
                            assert s == (sum(payload) & 0xFFFF), \
                                "checksum mismatch: %04x != %04x" % (s, sum(payload))
                            self.buf = self.buf[need:]
                            self.frames += 1
                            return tag, payload
            self._fill(timeout)


def nc_payload(oid, kind, path=b"", stop=False, flags=0):
    p = bytearray(64)
    struct.pack_into("<H", p, 8, oid)
    struct.pack_into("<H", p, 10, kind)
    struct.pack_into("<I", p, 12, flags)
    struct.pack_into("<I", p, 24, 1)
    if stop:
        struct.pack_into("<I", p, 32, 0xFFFFFFFF)
    if path:
        assert len(path) <= 32
        p[32:32 + len(path)] = path
    return bytes(p)


class Session:
    def __init__(self):
        self.sock = socket.create_connection((HOST, 2000), timeout=8)
        self.r = Reader(self.sock)

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass

    def send(self, tag, payload):
        self.sock.sendall(frame(tag, payload))

    def note(self, msg):
        print("   " + msg)

    def hello(self):
        self.send(b"CP", struct.pack("<I", 0) + bytes([0x06, 0x08, 0, 0]))
        tag, p = self.r.read_frame()
        assert tag == b"CP" and len(p) == 8 and p[4:] == bytes([0x06, 0x09, 0, 0]), \
            "hello reply unexpected: %s %s" % (tag, p.hex())
        self.note("hello ok: 0608 -> 0609")

    @staticmethod
    def _time_groups():
        grp = lambda d: struct.pack("<5I", d.year, d.month, d.day, d.hour, d.minute)
        return grp(datetime.now(timezone.utc)), grp(datetime.now().astimezone())

    def time_sync(self, oid):
        """k=1 dance from the capture: NC(flags=0x40000000), echo, CP-68 clock
        write, optional ACK0, response header; ack header, read data."""
        self.send(b"NC", nc_payload(oid, 1, flags=0x40000000))
        tag, echo = self.r.read_frame()
        g_utc, g_loc = self._time_groups()
        p = bytearray(68)
        p[8:28] = g_utc
        p[40:60] = g_loc
        self.send(b"CP", bytes(p))
        tag, nxt = self.r.read_frame()
        if tag == b"CP" and len(nxt) == 4:
            tag, hdr = self.r.read_frame()  # skip optional ACK0
        else:
            hdr = nxt
        hlen = struct.unpack("<I", hdr[16:20])[0]
        self.note("time_sync(0x%02x): echo=%04x hdr.len=%d status=%04x"
                  % (oid, struct.unpack("<I", echo[24:28])[0], hlen,
                     struct.unpack("<I", hdr[24:28])[0]))
        data = None
        if hlen:
            self.send(b"CP", b"\0\0\0\0")  # ack header; data follows
            tag, data = self.r.read_frame()
            assert len(data) == hlen + 4
        return hdr, data

    def op(self, oid, kind, path=b"", stop=False):
        """Standard op: NC -> echo(0x0A09) -> hdr -> ack -> data."""
        self.send(b"NC", nc_payload(oid, kind, path=path, stop=stop))
        tag, echo = self.r.read_frame()
        est = struct.unpack("<I", echo[24:28])[0]
        tag, hdr = self.r.read_frame()
        hlen = struct.unpack("<I", hdr[16:20])[0]
        status = struct.unpack("<I", hdr[24:28])[0]
        data = None
        if hlen:
            self.send(b"CP", b"\0\0\0\0")  # ack header before data arrives
            tag, data = self.r.read_frame()
            assert len(data) == hlen + 4
        self.note("op(0x%02x,%d): echo=%04x hdr.len=%d status=%04x data=%s"
                  % (oid, kind, est, hlen, status,
                     ("%dB" % (hlen - 4)) if data is not None else "none"))
        return echo, hdr, data

    def open_file(self, path):
        """NC(2,4): echo, hdr (size), ack, device pushes chunk 0."""
        self.send(b"NC", nc_payload(2, 4, path=path))
        tag, echo = self.r.read_frame()
        tag, hdr = self.r.read_frame()
        size = struct.unpack("<I", hdr[16:20])[0]
        status = struct.unpack("<I", hdr[24:28])[0]
        self.note("open %r -> size=%d status=%04x" % (path.decode(), size, status))
        chunks = {}
        if size:
            self.send(b"CP", b"\0\0\0\0")  # ack header; chunk 0 follows
            off, chunk = self.read_chunk()
            chunks[off] = chunk
        return size, chunks

    def read_chunk(self, timeout=10):
        tag, data = self.r.read_frame(timeout=timeout)
        return struct.unpack("<I", data[:4])[0], data[4:]

    def fetch_rest(self, size, chunks):
        """Request successive offsets until size bytes collected."""
        while sum(len(c) for c in chunks.values()) < size:
            self.send(b"CP", struct.pack("<I", sum(len(c) for c in chunks.values())))
            off, chunk = self.read_chunk()
            chunks[off] = chunk
        return b"".join(chunks[o] for o in sorted(chunks))


def save(name, blob):
    os.makedirs(OUT, exist_ok=True)
    path = os.path.join(OUT, name)
    with open(path, "wb") as f:
        f.write(blob)
    print("   saved %s (%d B)" % (path, len(blob)))
    return path


def udp_descriptor():
    u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    u.settimeout(3)
    u.sendto(b"aim-ka", (HOST, 36002))
    d, _ = u.recvfrom(2048)
    print("   UDP descriptor: %d B" % len(d))
    return d


def mode_probe():
    print("[probe]")
    s = Session()
    s.hello()
    _, blob = s.time_sync(0x10)
    save("info.bin", blob)
    s.time_sync(0x06)
    _, _, users = s.op(0x24, 6)
    save("users.bin", users[4:])
    _, _, cur = s.op(3, 2)
    print("   current-user payload: %s" % (cur[4:].hex() if cur else None))
    _, _, csv = s.op(0x24, 2)
    save("filelist.csv", csv[4:])
    rows = [r for r in csv[4:].decode("latin1").split("\r\n") if r and r[0].isalpha()]
    print("   file list rows: %d; first=%r last=%r"
          % (len(rows) - 1, rows[1].split(",")[0], rows[-1].split(",")[0]))
    _, _, cat = s.op(2, 2)
    save("catalog.bin", cat[4:])
    _, _, tree = s.op(8, 2)
    save("tree.bin", tree[4:])
    _, _, schema = s.op(9, 2)
    save("schema.bin", schema[4:])
    save("udp_descriptor.bin", udp_descriptor())
    s.close()
    print("[probe] frames read: %d, all checksums ok" % s.r.frames)


def mode_splash():
    print("[splash]")
    for path in (b"0:/log/splash.bmp", b"0:/lgo/splash.bmp"):
        s = Session()
        s.hello()
        s.time_sync(0x10)
        s.time_sync(0x06)
        size, chunks = s.open_file(path)
        if size:
            blob = s.fetch_rest(size, chunks)
            save("splash_%s.bmp" % path.decode().split("/")[-2], blob)
            print("   downloaded %d B, head=%s" % (len(blob), blob[:8].hex()))
        s.close()


def mode_live(seconds):
    print("[live %ds]" % seconds)
    s = Session()
    s.hello()
    s.time_sync(0x10)
    s.time_sync(0x06)
    s.op(0x24, 6)
    s.op(3, 2)
    s.op(0x53, 2)
    s.op(2, 2)
    s.op(8, 2)
    s.op(9, 2)
    s.op(0x28, 6)  # start stream
    index, raw, polls = [], bytearray(), 0
    t_end = time.time() + seconds
    try:
        while time.time() < t_end:
            for oid in (3, 0x53):
                s.send(b"NC", nc_payload(oid, 2))
                tag, echo = s.r.read_frame()
                tag, hdr = s.r.read_frame()
                hlen = struct.unpack("<I", hdr[16:20])[0]
                if hlen:
                    s.send(b"CP", b"\0\0\0\0")
                    tag, data = s.r.read_frame()
                    index.append({"oid": oid, "len": len(data), "t": time.time()})
                    raw += struct.pack("<II", oid, len(data)) + data
                polls += 1
            time.sleep(0.11)
    finally:
        try:
            s.op(0x51, 2, stop=True)  # stop stream
            _, hdr, data = s.op(3, 2)  # post-stop behavior of object 0x03
            if data is not None:
                print("   post-stop 0x03 -> %d B: %s" % (len(data) - 4, data[4:20].hex()))
        except Exception as e:
            print("   stop/post-stop failed: %r" % e)
        s.close()
    save("live_frames.bin", bytes(raw))
    save("live_index.json", json.dumps(index).encode())
    print("   polls=%d frames=%d" % (polls, len(index)))


def mode_download(name):
    print("[download %s]" % name)
    s = Session()
    s.hello()
    path = name.encode() if name.startswith(("0:", "1:")) else b"1:/mem/" + name.encode()
    size, chunks = s.open_file(path)
    blob = s.fetch_rest(size, chunks)
    print("   downloaded %d B (expected %d), offsets=%#x..%#x"
          % (len(blob), size, min(chunks), max(chunks)))
    assert blob[:1] == b"\x78", "not zlib: %s" % blob[:4].hex()
    raw = zlib.decompress(blob)
    base = os.path.basename(path.decode())
    save(base, blob)
    save(base + ".inflated", raw)
    print("   zlib ok, inflated %d B, head=%r" % (len(raw), raw[:12]))
    s.close()


def mode_objprobe(oid, kind, path=b""):
    print("[objprobe id=0x%02x kind=%d]" % (oid, kind))
    s = Session()
    s.hello()
    s.time_sync(0x10)
    s.time_sync(0x06)
    try:
        _, hdr, data = s.op(oid, kind, path=path)
        if data is not None:
            head = data[4:36].hex()
            txt = bytes(b for b in data[4:100] if 32 <= b < 127)
            print("   data head=%s ascii=%r" % (head, txt))
    except (AssertionError, EOFError, socket.timeout, OSError) as e:
        print("   probe failed: %r" % e)
    finally:
        s.close()


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "probe"
    if mode == "probe":
        mode_probe()
    elif mode == "splash":
        mode_splash()
    elif mode == "live":
        mode_live(int(sys.argv[2]) if len(sys.argv) > 2 else 10)
    elif mode == "download":
        mode_download(sys.argv[2])
    elif mode == "objprobe":
        mode_objprobe(int(sys.argv[2], 0), int(sys.argv[3], 0),
                      sys.argv[4].encode() if len(sys.argv) > 4 else b"")
    else:
        print(__doc__)


if __name__ == "__main__":
    main()
