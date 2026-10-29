import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import AsyncMock,MagicMock,patch

from engine import Engine
from station_client import audio_command
spec=importlib.util.spec_from_file_location('webhook',ROOT/'cloud/app.py')
webhook=importlib.util.module_from_spec(spec);spec.loader.exec_module(webhook)

class Commands(unittest.TestCase):
 def test_voice_commands(self):
  for text,action in [('следующая','next'),('пауза','pause'),('продолжи музыку','resume'),('включи похожие','similar'),('что играет?','now')]:
   self.assertEqual(webhook.parse(text),(action,''))
 def test_current_song_phrases(self):
  for text in ['что за песня сейчас играет','Что за песня?','какая песня сейчас играет','что за трек','кто поёт']:
   self.assertEqual(webhook.parse(text),('now',''))
 def test_infinitive_request(self):
  self.assertEqual(webhook.parse('включить Babydoll Dominic Fike'),('play','Babydoll Dominic Fike'))
 def test_conversation_stays_open(self):
  with webhook.app.app_context():self.assertFalse(webhook.answer('Продолжаю.').json['response']['end_session'])
 def test_explicit_exit(self):
  with webhook.app.app_context():self.assertTrue(webhook.answer('Выход.',True).json['response']['end_session'])
 def test_song_name_preserved(self):
  self.assertEqual(webhook.parse('Включи песню Babydoll Dominic Fike'),('play','Babydoll Dominic Fike'))
 def test_wrong_secret_rejected(self):
  with webhook.app.test_client() as c:self.assertEqual(c.post('/alice/invalid',json={}).status_code,404)
 def test_agent_auth(self):
  with webhook.app.test_client() as c:self.assertEqual(c.post('/agent',json={}).status_code,403)
 def test_audio_protocol(self):
  import base64
  command=audio_command('http://local/audio',{'title':'Песня','artist':'Автор'},'id')
  wire=base64.b64decode(command['data'])
  self.assertIn(b'audio_play',wire);self.assertNotIn(b'radio_play',wire)
  self.assertIn(b'http://local/audio',wire)

class WebhookRouting(unittest.TestCase):
 def setUp(self):
  os.environ.update(WEBHOOK_TOKEN='test-hook',ALICE_SKILL_ID='test-skill',GOOGLE_CLOUD_PROJECT='test-project')
  self.body={'session':{'skill_id':'test-skill','session_id':'s','message_id':1,
                       'application':{'application_id':'station'},'user':{'user_id':'owner'}},
             'request':{'type':'SimpleUtterance','command':'следующая'}}
  self.database=MagicMock();webhook._db=self.database;self.pub=MagicMock();webhook._publisher=self.pub
  self.route=MagicMock();self.route.exists=True;self.route.to_dict.return_value={'owner':webhook.owner(self.body),'station':'home'}
  self.agent=MagicMock();self.agent.exists=True;self.agent.to_dict.return_value={'last_seen':time.time(),'status':{'station_connected':True}}
  self.job=MagicMock()
  def collection(name):
   obj=MagicMock();obj.document.return_value={'music_surfaces':self.route,'music_state':self.agent,'music_jobs':self.job}[name]
   return obj
  self.database.collection.side_effect=collection
  self.route.get.return_value=self.route;self.agent.get.return_value=self.agent
 def tearDown(self):webhook._db=None;webhook._publisher=None
 def send(self):
  with webhook.app.test_client() as c:return c.post('/alice/test-hook',json=self.body)
 def test_launch_is_brief(self):
  self.body['request']['command']=''
  response=self.send().json['response']
  self.assertEqual(response['text'],'Что включить?');self.assertFalse(response['end_session'])
 def test_greeting_on_startup_is_brief(self):
  self.body['request']['command']='скажи навыку моя фонотека привет'
  self.assertEqual(self.send().json['response']['text'],'Что включить?')
 def test_next_ack_is_brief(self):
  self.assertEqual(self.send().json['response']['text'],'Дальше.')
 def test_current_song_returns_title_and_requests_only_current_stream_recovery(self):
  self.body['request']['command']='что за песня сейчас играет'
  self.agent.to_dict.return_value={'last_seen':time.time(),'status':{'station_connected':True,'current':{'title':'Song','artist':'Artist'}}}
  self.assertEqual(self.send().json['response']['text'],'Song, Artist')
  payload=json.loads(self.pub.publish.call_args.args[1])
  self.assertEqual(payload['action'],'now');self.assertEqual(payload['query'],'')
 def test_unbound_station_cannot_publish(self):
  self.route.exists=False
  self.assertIn('не подключена',self.send().json['response']['text']);self.pub.publish.assert_not_called()
 def test_other_owner_cannot_publish(self):
  self.route.to_dict.return_value={'owner':'another-owner'}
  self.send();self.pub.publish.assert_not_called()
 def test_offline_station_cannot_publish(self):
  self.agent.to_dict.return_value={'last_seen':time.time(),'status':{'station_connected':False}}
  self.assertIn('Не вижу',self.send().json['response']['text']);self.pub.publish.assert_not_called()
 def test_retry_republishes_same_job_id(self):
  from google.api_core.exceptions import AlreadyExists
  self.job.create.side_effect=AlreadyExists('retry')
  self.send();first=json.loads(self.pub.publish.call_args.args[1])['id']
  self.send();second=json.loads(self.pub.publish.call_args.args[1])['id']
  self.assertEqual(first,second);self.assertEqual(self.pub.publish.call_count,2)


class Queue(unittest.IsolatedAsyncioTestCase):
 async def asyncSetUp(self):
  self.temp=tempfile.TemporaryDirectory();self.path=Path(self.temp.name)
  self.engine=Engine({},self.path,self.path,None)
  self.engine.current={'id':'1','title':'Song','artist':'Artist','duration':100};self.engine.duration=100
  self.engine.state='playing';self.engine.confirmed=True;self.engine.last_playing=True
  self.engine.fetch_before=0;self.engine.fetch_count=1;self.engine.grace=0
 async def asyncTearDown(self):
  if self.engine.transition:await self.engine.transition
  self.temp.cleanup()
 async def test_natural_end_advances(self):
  e=self.engine;e.last_progress=99;e.auto_next=AsyncMock()
  await e.on_state({'playing':False,'aliceState':'IDLE'});await e.transition
  e.auto_next.assert_awaited_once()
 async def test_manual_pause_does_not_advance(self):
  e=self.engine;e.last_progress=20;e.auto_next=AsyncMock()
  await e.on_state({'playing':False,'aliceState':'IDLE'})
  self.assertEqual(e.state,'paused');e.auto_next.assert_not_called()
 async def test_voice_interrupt_does_not_advance(self):
  e=self.engine;e.last_progress=99
  await e.on_state({'playing':False,'aliceState':'LISTENING'})
  self.assertIsNone(e.transition)
 async def test_unfetched_track_does_not_advance(self):
  e=self.engine;e.last_progress=99;e.fetch_count=0
  await e.on_state({'playing':False,'aliceState':'IDLE'})
  self.assertIsNone(e.transition)
 async def test_similar_tracks_deduplicated(self):
  e=self.engine;e.seed='1';e.seen={'1'};e.upcoming=[]
  e.apple.similar=AsyncMock(return_value=[{'id':'1'},{'id':'2'},{'id':'2'}])
  await e.refill();self.assertEqual(e.upcoming,[{'id':'2'}])
 async def test_failed_next_preserves_queue(self):
  e=self.engine;e.upcoming=[{'id':'2'}];e.play_track=AsyncMock(side_effect=RuntimeError('busy'))
  with self.assertRaises(RuntimeError):await e.next()
  self.assertEqual(e.upcoming,[{'id':'2'}])
 async def run_voice_recovery(self,volume=.4,stream='stream-1',generation=0,resume_if_stopped=False):
  import itertools
  e=self.engine;e.stream_id='stream-1'
  e.station.send=AsyncMock(return_value={})
  with patch('engine.asyncio.sleep',new_callable=AsyncMock),patch('engine.time.monotonic',side_effect=itertools.count(100)):
   await e.recover_after_voice(generation,stream,volume,resume_if_stopped=resume_if_stopped)
  return [c for c in e.station.send.call_args_list if c.args[0].get('command')!='softwareVersion']
 async def test_current_song_question_recovers_playing_track(self):
  e=self.engine;e.stream_id='same-song';e.recover_after_voice=AsyncMock()
  await e.command('now',voice=True);await e.voice_recovery
  e.recover_after_voice.assert_awaited_once()
  self.assertEqual(e.current['id'],'1')
 async def test_current_song_question_preserves_intentional_pause(self):
  e=self.engine;e.state='paused';e.recover_after_voice=AsyncMock()
  await e.command('now',voice=True)
  e.recover_after_voice.assert_not_awaited();self.assertIsNone(e.voice_recovery)
 async def test_audio_waits_until_listening_is_finished(self):
  import itertools
  e=self.engine;e.voice_not_before=102;e.station_state={'aliceState':'LISTENING'};e.station.send=AsyncMock(return_value={})
  waits=[]
  async def tick(delay):
   waits.append(delay)
   if len(waits)>=4:e.station_state={'aliceState':'IDLE'}
  with patch('engine.asyncio.sleep',side_effect=tick),patch('engine.time.monotonic',side_effect=itertools.count(100)):
   await e.wait_voice_reply()
  self.assertGreaterEqual(len(waits),4);self.assertEqual(e.voice_not_before,0)
 async def test_wait_uses_fresh_station_state_instead_of_old_push_event(self):
  import itertools
  e=self.engine;e.voice_not_before=102;e.station_state={'aliceState':'LISTENING'}
  e.station.send=AsyncMock(return_value={'state':{'aliceState':'IDLE'}})
  with patch('engine.asyncio.sleep',new_callable=AsyncMock),patch('engine.time.monotonic',side_effect=itertools.count(100)):
   await e.wait_voice_reply()
  self.assertEqual(e.station_state['aliceState'],'IDLE')
  e.station.send.assert_awaited_with({'command':'softwareVersion'})
 async def test_audio_does_not_start_if_alice_never_finishes(self):
  import itertools
  e=self.engine;e.voice_not_before=102;e.station_state={'aliceState':'LISTENING'};e.station.send=AsyncMock(return_value={})
  with patch('engine.asyncio.sleep',new_callable=AsyncMock),patch('engine.time.monotonic',side_effect=itertools.count(100)):
   with self.assertRaisesRegex(RuntimeError,'ещё слушает'):await e.wait_voice_reply()
 async def test_voice_reply_restores_same_stream_and_ducked_volume(self):
  self.engine.station_state={'aliceState':'IDLE','playerState':{'id':'stream-1'},'volume':.2}
  calls=await self.run_voice_recovery()
  self.assertEqual([c.args[0] for c in calls],[{'command':'setVolume','volume':.4}])
 async def test_voice_recovery_never_toggles_an_already_playing_stream(self):
  self.engine.station_state={'aliceState':'IDLE','playing':True,'playerState':{'id':'stream-1'},'volume':.4}
  self.assertEqual(await self.run_voice_recovery(),[])
 async def test_playing_stream_only_restores_ducked_volume(self):
  self.engine.station_state={'aliceState':'IDLE','playing':True,'playerState':{'id':'stream-1'},'volume':.2}
  calls=await self.run_voice_recovery()
  self.assertEqual([c.args[0] for c in calls],[{'command':'setVolume','volume':.4}])
 async def test_resume_is_idempotent_while_already_playing(self):
  e=self.engine;e.station_state={'playing':True};e.station.send=AsyncMock(return_value={})
  await e.resume();self.assertFalse(any(c.args[0].get('command')=='play' for c in e.station.send.call_args_list))
 async def test_title_question_resumes_a_stopped_stream_once(self):
  self.engine.station_state={'aliceState':'IDLE','playing':False,'playerState':{'id':'stream-1'},'volume':.4}
  self.engine.resume=AsyncMock()
  await self.run_voice_recovery(resume_if_stopped=True)
  self.engine.resume.assert_awaited_once()
 async def test_paused_resume_reopens_same_track_at_saved_position(self):
  e=self.engine;e.station_state={'aliceState':'IDLE','playing':False};e.position=25
  e.station.send=AsyncMock(return_value={});e.play_track=AsyncMock()
  with patch('engine.asyncio.sleep',new_callable=AsyncMock):await e.resume()
  e.play_track.assert_awaited_once_with(e.current)
  self.assertIn({'command':'rewind','position':25},[c.args[0] for c in e.station.send.call_args_list])
  self.assertFalse(any(c.args[0].get('command')=='play' for c in e.station.send.call_args_list))
 async def test_resume_uses_fresh_playing_state(self):
  e=self.engine;e.station_state={'playing':False}
  e.station.send=AsyncMock(return_value={'state':{'playing':True}})
  await e.resume()
  self.assertEqual([c.args[0] for c in e.station.send.call_args_list],[{'command':'softwareVersion'}])
 async def test_voice_recovery_does_not_override_other_player(self):
  self.engine.station_state={'aliceState':'IDLE','playerState':{'id':'native-song'},'volume':.2}
  self.assertEqual(await self.run_voice_recovery(),[])
 async def test_stale_voice_recovery_cannot_restart_new_track(self):
  self.engine.generation=2
  self.assertEqual(await self.run_voice_recovery(generation=1),[])
 async def test_voice_recovery_preserves_user_volume_change(self):
  self.engine.station_state={'aliceState':'IDLE','playerState':{'id':'stream-1'},'volume':.33}
  calls=await self.run_voice_recovery()
  self.assertEqual([c.args[0] for c in calls],[])
 async def test_voice_recovery_preserves_mute(self):
  self.engine.station_state={'aliceState':'IDLE','playerState':{'id':'stream-1'},'volume':0}
  calls=await self.run_voice_recovery(volume=0)
  self.assertEqual([c.args[0] for c in calls],[])
 async def test_explicit_pause_invalidates_voice_recovery(self):
  e=self.engine;e.station.send=AsyncMock(return_value={});generation=e.generation
  e.voice_recovery=asyncio.create_task(asyncio.sleep(100))
  await e.pause();await asyncio.gather(e.voice_recovery,return_exceptions=True)
  self.assertGreater(e.generation,generation);self.assertEqual(e.state,'paused')
 async def test_queue_restores_without_spontaneous_play(self):
  e=self.engine;e.upcoming=[{'id':'2'}];e.position=37;e.checkpoint()
  restored=Engine({},self.path,self.path,None)
  self.assertEqual(restored.current['id'],'1');self.assertEqual(restored.position,37)
  self.assertEqual(restored.state,'paused');self.assertTrue(restored.restored)

if __name__=='__main__':unittest.main()
