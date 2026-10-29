"""Yield the local Apple Music queue to a connected AirPlay sender."""
import asyncio
import contextlib
import os


class AirplayMonitor:
    def __init__(self, engine, socket):
        self.engine, self.socket = engine, socket

    async def update(self, connected):
        previous=self.engine.airplay_active
        self.engine.airplay_active=bool(connected)
        if connected and not previous:
            if self.engine.state in ('playing','starting'):
                await self.engine.pause()
            print('Audio focus: AirPlay.',flush=True)
        elif previous and not connected:
            # Disconnecting a phone must not start the saved queue unexpectedly.
            print('Audio focus: local voice available.',flush=True)

    async def run(self):
        from dbus_next.aio import MessageBus
        from dbus_next import Message,MessageType
        while True:
            bus=None
            try:
                if not os.path.exists(self.socket):
                    await self.update(False);await asyncio.sleep(1);continue
                bus=MessageBus(bus_address='unix:path='+self.socket)
                await bus.connect()
                while True:
                    reply=await asyncio.wait_for(bus.call(Message(destination='org.gnome.ShairportSync',
                        path='/org/gnome/ShairportSync',interface='org.gnome.ShairportSync',member='GetControlStatus')),2)
                    if reply.message_type==MessageType.ERROR:raise ConnectionError('Receiver unavailable')
                    version,modern,client=reply.body
                    await self.update(version==1 and modern and bool(client))
                    await asyncio.sleep(.5)
            except asyncio.CancelledError:raise
            except Exception:
                await self.update(False);await asyncio.sleep(1)
            finally:
                if bus:
                    bus.disconnect()
                    with contextlib.suppress(Exception):
                        await asyncio.wait_for(bus.wait_for_disconnect(),1)
