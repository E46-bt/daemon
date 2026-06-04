#!/usr/bin/env python3
"""Bluetooth pairing agent — auto-accepts all requests (NoInputNoOutput / Just Works)."""
import dbus
import dbus.service
import dbus.mainloop.glib
from gi.repository import GLib

AGENT_PATH = '/bt/agent'
AGENT_IFACE = 'org.bluez.Agent1'


class PairingAgent(dbus.service.Object):

    @dbus.service.method(AGENT_IFACE, in_signature='ou', out_signature='')
    def RequestConfirmation(self, device, passkey):
        print(f'RequestConfirmation: {device} passkey={passkey:06d} — accepted', flush=True)

    @dbus.service.method(AGENT_IFACE, in_signature='o', out_signature='')
    def RequestAuthorization(self, device):
        print(f'RequestAuthorization: {device} — accepted', flush=True)

    @dbus.service.method(AGENT_IFACE, in_signature='os', out_signature='')
    def AuthorizeService(self, device, uuid):
        print(f'AuthorizeService: {device} uuid={uuid} — accepted', flush=True)
        try:
            props = dbus.Interface(bus.get_object('org.bluez', device),
                                   'org.freedesktop.DBus.Properties')
            props.Set('org.bluez.Device1', 'Trusted', dbus.Boolean(True))
            print(f'Trusted: {device}', flush=True)
        except Exception as e:
            print(f'Could not trust {device}: {e}', flush=True)

    @dbus.service.method(AGENT_IFACE, in_signature='o', out_signature='s')
    def RequestPinCode(self, device):
        return '0000'

    @dbus.service.method(AGENT_IFACE, in_signature='o', out_signature='u')
    def RequestPasskey(self, device):
        return dbus.UInt32(0)

    @dbus.service.method(AGENT_IFACE, in_signature='ou', out_signature='')
    def DisplayPasskey(self, device, passkey):
        pass

    @dbus.service.method(AGENT_IFACE, in_signature='os', out_signature='')
    def DisplayPinCode(self, device, pincode):
        pass

    @dbus.service.method(AGENT_IFACE, in_signature='', out_signature='')
    def Cancel(self):
        pass

    @dbus.service.method(AGENT_IFACE, in_signature='', out_signature='')
    def Release(self):
        pass


dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SystemBus()
manager = dbus.Interface(
    bus.get_object('org.bluez', '/org/bluez'),
    'org.bluez.AgentManager1',
)
agent = PairingAgent(bus, AGENT_PATH)
manager.RegisterAgent(AGENT_PATH, 'NoInputNoOutput')
manager.RequestDefaultAgent(AGENT_PATH)
print('Bluetooth pairing agent ready (NoInputNoOutput — auto-accept)', flush=True)
GLib.MainLoop().run()
