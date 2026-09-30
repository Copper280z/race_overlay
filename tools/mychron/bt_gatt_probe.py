#!/usr/bin/env python3
"""GATT enumeration and write-probe tool for AiM MyChron6 loggers.

Discovers the logger by BLE advertisement name prefix (MYC6), connects,
prints every service/characteristic with flags and readable values,
subscribes to all notifiable characteristics, and probes each writable
characteristic with the known STCP hello (framed and raw). See
docs/mychron-protocol.md for the protocol background.

Requires: python3 with bleak (any venv), Linux/BlueZ or macOS/Windows.
Usage: bt_gatt_probe.py [device_address]   # address optional; auto-discovers
"""
import asyncio, struct, sys, time
from bleak import BleakScanner, BleakClient

NAME_PREFIX = "MYC6"
HELLO = struct.pack("<I", 0) + bytes([0x06, 0x08, 0, 0])

def frame(tag, payload):
    return (b"<hST" + tag + struct.pack("<I", len(payload)) + b"\x00>"
            + payload + b"<ST" + tag + struct.pack("<H", sum(payload) & 0xFFFF) + b">")

events = []

def mk(name):
    def cb(_s, data):
        events.append((time.time(), name, bytes(data)))
        print("  NOTIFY[%s] %3d B %s" % (name, len(data), bytes(data).hex()[:80]))
    return cb

async def find_device(addr):
    if addr:
        return await BleakScanner.find_device_by_address(addr, timeout=15)
    print("discovering; picking first device named %r..." % NAME_PREFIX)
    devs = await BleakScanner.discover(timeout=12)
    for d in devs:
        if d.name and d.name.startswith(NAME_PREFIX):
            return d
    return None

async def main():
    addr = sys.argv[1] if len(sys.argv) > 1 else None
    dev = await find_device(addr)
    if not dev:
        print("logger not found (is Bluetooth enabled and not already connected?)")
        raise SystemExit(2)
    print("found:", dev.name, dev.address)
    async with BleakClient(dev, timeout=20) as client:
        print("connected, mtu:", client.mtu_size)
        writers, readers = [], []
        for s in client.services:
            print("SERVICE", s.uuid)
            for c in s.characteristics:
                print("  CHAR %s flags=%s" % (c.uuid, ",".join(c.properties)))
                if "read" in c.properties:
                    readers.append(c)
                if "write" in c.properties or "write-without-response" in c.properties:
                    writers.append(c)
        print("\n== reads ==")
        for c in readers:
            try:
                v = await client.read_gatt_char(c)
                print("READ %s -> %s %r" % (c.uuid[-12:], v[:48].hex(),
                                            bytes(b for b in v if 32 <= b < 127)))
            except Exception as e:
                print("READ %s failed: %s" % (c.uuid[-12:], type(e).__name__))
        print("\n== notify subscribe ==")
        for s in client.services:
            for c in s.characteristics:
                if "notify" in c.properties or "indicate" in c.properties:
                    try:
                        await client.start_notify(c, mk(c.uuid[-12:]))
                        print("notify on", c.uuid[-12:])
                    except Exception as e:
                        print("notify fail", c.uuid[-12:], repr(e))
        await asyncio.sleep(0.3)
        print("\n== write probes ==")
        for c in writers:
            for label, payload in (("framed-hello", frame(b"CP", HELLO)),
                                   ("raw-hello", HELLO)):
                try:
                    await client.write_gatt_char(c, payload, response=True)
                    print("WROTE %s/%s %d B" % (c.uuid[-12:], label, len(payload)))
                except Exception as e:
                    print("write %s/%s failed: %s" % (c.uuid[-12:], label, type(e).__name__))
                await asyncio.sleep(1.6)
        await asyncio.sleep(4)
    print("notifications total:", len(events))

asyncio.run(main())
