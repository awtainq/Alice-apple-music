import sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock
from airplay import AirplayMonitor
from voice import Voice

class Focus(unittest.IsolatedAsyncioTestCase):
 async def test_connect_pauses_local_queue_once(self):
  e=SimpleNamespace(airplay_active=False,state='playing',pause=AsyncMock())
  monitor=AirplayMonitor(e,'unused')
  await monitor.update(True);await monitor.update(True)
  self.assertTrue(e.airplay_active);e.pause.assert_awaited_once()
 async def test_disconnect_does_not_resume_saved_music(self):
  e=SimpleNamespace(airplay_active=True,state='paused',pause=AsyncMock(),resume=AsyncMock())
  await AirplayMonitor(e,'unused').update(False)
  self.assertFalse(e.airplay_active);e.resume.assert_not_awaited()
 async def test_transport_does_not_run_both_queues(self):
  v=Voice({},None);v.status=AsyncMock(return_value={'airplay_active':True,'current':{'id':'1'}})
  v.ipc_writer=SimpleNamespace(write=AsyncMock(),drain=AsyncMock())
  await v.accept(3,'дальше',source='spotter')
  self.assertTrue(v.queue.empty());v.ipc_writer.write.assert_not_called()
