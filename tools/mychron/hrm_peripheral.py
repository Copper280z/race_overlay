#!/usr/bin/env python3
"""Fake BLE heart-rate monitor (BlueZ peripheral) for MyChron6 BT-link testing.
Advertises PROBE-HRM with Heart Rate Service (0x180D) + Battery (0x180F).
Logs every GATT read/subscribe; notifies a cycling HR value while subscribed."""
import time
import dbus, dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

BLUEZ = 'org.bluez'
GATT_MGR = 'org.bluez.GattManager1'
ADV_MGR = 'org.bluez.LEAdvertisingManager1'
SVC_IFACE = 'org.bluez.GattService1'
CHRC_IFACE = 'org.bluez.GattCharacteristic1'
ADV_IFACE = 'org.bluez.LEAdvertisement1'
OM_IFACE = 'org.freedesktop.DBus.ObjectManager'
PROPS_IFACE = 'org.freedesktop.DBus.Properties'

def log(msg):
    print("[%s] %s" % (time.strftime("%H:%M:%S"), msg), flush=True)

HR_UUID = '00002a37-0000-1000-8000-00805f9b34fb'
BSL_UUID = '00002a38-0000-1000-8000-00805f9b34fb'
BAT_UUID = '00002a19-0000-1000-8000-00805f9b34fb'
HRS_UUID = '0000180d-0000-1000-8000-00805f9b34fb'
BAS_UUID = '0000180f-0000-1000-8000-00805f9b34fb'

state = {'hr': 100, 'notifying': False}
HR_CYCLE = [100, 128, 91, 152, 117, 143]


class Advertisement(dbus.service.Object):
    def __init__(self, bus, index):
        self.path = '/org/bluez/probeadv/advertisement%d' % index
        dbus.service.Object.__init__(self, bus, self.path)

    def get_props(self):
        return {ADV_IFACE: {
            'Type': 'peripheral',
            'ServiceUUIDs': dbus.Array([HRS_UUID, BAS_UUID], signature='s'),
            'LocalName': dbus.String('PROBE-HRM'),
            'Appearance': dbus.UInt16(833),
            'IncludeTxPower': dbus.Boolean(True),
        }}

    @dbus.service.method(OM_IFACE, out_signature='a{oa{sa{sv}}}')
    def get_props(self):
        return {ADV_IFACE: {
            'Type': 'peripheral',
            'ServiceUUIDs': dbus.Array([HRS_UUID], signature='s'),
            'LocalName': dbus.String('HRM'),
            'Appearance': dbus.UInt16(833),
        }}

class Service(dbus.service.Object):
    def __init__(self, bus, index, uuid, primary):
        self.path = '/org/bluez/probe/service%d' % index
        self.uuid = uuid
        self.primary = primary
        self.chars = []
        dbus.service.Object.__init__(self, bus, self.path)

    def get_props(self):
        return {SVC_IFACE: {
            'UUID': dbus.String(self.uuid),
            'Primary': dbus.Boolean(self.primary),
            'Includes': dbus.Array([], signature='o'),
        }}


class Characteristic(dbus.service.Object):
    def __init__(self, bus, uuid, flags, service, value):
        self.path = service.path + '/char%s' % uuid[4:8]
        self.uuid = uuid
        self.flags = flags
        self.service = service
        self.value = bytearray(value)
        dbus.service.Object.__init__(self, bus, self.path)

    def get_props(self):
        return {CHRC_IFACE: {
            'UUID': dbus.String(self.uuid),
            'Service': dbus.ObjectPath(self.service.path),
            'Flags': dbus.Array(self.flags, signature='s'),
        }}

    @dbus.service.method(PROPS_IFACE, in_signature='s', out_signature='a{sv}')
    def GetAll(self, interface):
        if interface != CHRC_IFACE:
            raise dbus.exceptions.DBusException('org.bluez.Error.InvalidArguments')
        return self.get_props()[CHRC_IFACE]

    @dbus.service.method(PROPS_IFACE, in_signature='ss', out_signature='v')
    def Get(self, interface, name):
        return self.GetAll(interface)[name]

    @dbus.service.method(CHRC_IFACE, in_signature='a{sv}', out_signature='ay')
    def ReadValue(self, options):
        log('ReadValue %s -> %s' % (self.uuid[4:8], bytes(self.value).hex()))
        return dbus.Array(self.value, signature='y')

    @dbus.service.method(CHRC_IFACE, in_signature='', out_signature='')
    def StartNotify(self):
        state['notifying'] = True
        log('StartNotify %s (central subscribed)' % self.uuid[4:8])

    @dbus.service.method(CHRC_IFACE, in_signature='', out_signature='')
    def StopNotify(self):
        state['notifying'] = False
        log('StopNotify %s' % self.uuid[4:8])

    def notify(self, value):
        self.value = bytearray(value)
        self.PropertiesChanged(CHRC_IFACE,
                               {'Value': dbus.Array(self.value, signature='y')},
                               [])


class Application(dbus.service.Object):
    def __init__(self, bus):
        self.path = '/org/bluez/probe'
        self.services = []
        dbus.service.Object.__init__(self, bus, self.path)

    @dbus.service.method(OM_IFACE, out_signature='a{oa{sa{sv}}}')
    def GetManagedObjects(self):
        objs = {}
        for svc in self.services:
            objs[svc.path] = svc.get_props()
            for ch in svc.chars:
                objs[ch.path] = ch.get_props()
        log('GetManagedObjects called -> %d objects: %s'
            % (len(objs), ', '.join(sorted(objs))))
        return objs


def main():
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SystemBus()

    app = Application(bus)
    hrs = Service(bus, 0, HRS_UUID, True)
    meas = Characteristic(bus, HR_UUID, ['notify'], hrs, [0x00, 100])
    bsl = Characteristic(bus, BSL_UUID, ['read'], hrs, [0x01])
    hrs.chars = [meas, bsl]
    bas = Service(bus, 1, BAS_UUID, True)
    bat = Characteristic(bus, BAT_UUID, ['read', 'notify'], bas, [90])
    bas.chars = [bat]
    app.services = [hrs, bas]

    adapter = bus.get_object(BLUEZ, '/org/bluez/hci0')

    adv = Advertisement(bus, 0)
    empty = dbus.Dictionary({}, signature='sv')
    dbus.Interface(adapter, ADV_MGR).RegisterAdvertisement(
        adv.path, empty,
        reply_handler=lambda: log('advertisement registered'),
        error_handler=lambda e: log('advertisement FAILED: %s' % e))

    dbus.Interface(adapter, GATT_MGR).RegisterApplication(
        app.path, empty,
        reply_handler=lambda: log('GATT application registered'),
        error_handler=lambda e: log('GATT application FAILED: %s' % e))

    def tick():
        if state['notifying']:
            i = HR_CYCLE.index(state['hr'])
            state['hr'] = HR_CYCLE[(i + 1) % len(HR_CYCLE)]
            meas.notify([0x00, state['hr']])
            bat.notify([90])
            log('notified hr=%d' % state['hr'])
        return True

    GLib.timeout_add_seconds(1, tick)
    log('PROBE-HRM peripheral running')
    GLib.MainLoop().run()


if __name__ == '__main__':
    main()
