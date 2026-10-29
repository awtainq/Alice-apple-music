import sys
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import asyncio
import struct
import unittest
from unittest.mock import AsyncMock, MagicMock
from voice import fields, recognition, parse_phrase, Voice, MAX_FRAME, cancel_frame
from station_client import varint
from apple import Apple


def frame(state=3, text='дальше'):
 body=b'\x08'+varint(state)+b'\x12'+varint(len(text.encode()))+text.encode()
 return varint(253*8)+b'\x03'+varint(252*8+2)+varint(len(body))+body

class Recognition(unittest.TestCase):
 def test_final_text(self):self.assertEqual(recognition(frame()),(3,'дальше'))
 def test_interim_not_forwarded(self):self.assertEqual(recognition(frame(1,'секрет')),(1,''))
 def test_idle_omitted_default(self):
  self.assertEqual(recognition(varint(253*8)+b'\x03'+varint(252*8+2)+b'\x00'),(0,''))
 def test_malformed_frames(self):
  for value in (b'',b'\xff'*20,b'\x00',frame()[:-1],b'\x0a\xff\x7f'):
   self.assertIsNone(recognition(value))
 def test_other_message_type(self):self.assertIsNone(recognition(b'\x08\x03'))
 def test_cancel_requests_only_local_dialog_cancellation(self):
  wire=cancel_frame();methods=[]
  while wire:
   size=struct.unpack('<I',wire[:4])[0];body=wire[4:4+size];wire=wire[4+size:]
   self.assertEqual(size,len(body));outer=dict(fields(body));self.assertEqual(outer[253],210)
   remote=dict(fields(outer[252]));self.assertEqual(remote[7],b'AliceCapability')
   method=dict(fields(remote[8]));self.assertEqual(method[7],1);methods.append(method[1])
  self.assertEqual(methods,[3,6,14])
 def test_voice_modes(self):
  for text,expected in [('Алиса, включи песню Babydoll!',('play','babydoll')),
    ('поставь альбом Nevermind',('album','nevermind')),
    ('включи плейлист Для сна',('playlist','для сна')),
    ('Включи Noize MC',('auto','noize mc')),
    ('поставь музыку',('library','')),
    ('Включи исполнителя Мадонна',('artist','мадонна')),
    ('включи похожие',('similar','')),
    ('Алиса, что за песня сейчас играет?',('announce_now',''))]:
   self.assertEqual(parse_phrase(text,True),expected)
 def test_controls_require_recent_music(self):
  for text in ['дальше','назад','пауза','продолжай','что играет']:
   self.assertIsNone(parse_phrase(text,False))
 def test_household_commands_not_intercepted(self):
  for text in ['включи свет','поставь таймер на 5 минут','включи радио','включи телевизор',
   'поставь будильник на семь','скажи навыку моя фонотека дальше','выйди из навыка','громче','какая погода']:
   self.assertIsNone(parse_phrase(text,True))
 def test_explicit_song_may_contain_device_word(self):
  self.assertEqual(parse_phrase('включи песню свет'),('play','свет'))
 def test_polite_suffix(self):self.assertEqual(parse_phrase('включи песню Hello, пожалуйста'),('play','hello'))

class Accept(unittest.IsolatedAsyncioTestCase):
 async def asyncSetUp(self):
  self.v=Voice({},None);self.v.status=AsyncMock(return_value={'state':'playing','station_stream_id':'alice-music-light:1:1'})
 async def test_one_event_per_recognition_cycle(self):
  await self.v.accept(3,'дальше');await self.v.accept(3,'дальше')
  self.assertEqual(self.v.queue.qsize(),1)
  await self.v.accept(1,'');await self.v.accept(3,'дальше')
  self.assertEqual(self.v.queue.qsize(),2)
 async def test_unrelated_speech_not_sent_to_player(self):
  self.v.ipc_writer=MagicMock();self.v.ipc_writer.drain=AsyncMock()
  await self.v.accept(3,'мое личное сообщение');self.v.status.assert_not_awaited()
  self.v.ipc_writer.write.assert_not_called()
 async def test_only_accepted_music_cancels_builtin_reply(self):
  self.v.ipc_writer=MagicMock();self.v.ipc_writer.drain=AsyncMock()
  await self.v.accept(3,'дальше')
  self.v.ipc_writer.write.assert_called_once_with(cancel_frame())
 async def test_first_hour_after_boot_is_not_previous_music_activity(self):
  from unittest.mock import patch
  self.v.status.return_value={'state':'stopped','current':None}
  with patch('voice.time.monotonic',return_value=100):
   await self.v.accept(3,'дальше',source='spotter')
  self.assertTrue(self.v.queue.empty())
 async def test_builtin_player_not_claimed(self):
  self.v.status.return_value={'state':'playing','station_stream_id':'yandex:other'}
  await self.v.accept(3,'дальше');self.assertTrue(self.v.queue.empty())
 async def test_bare_next_restores_apple_queue_after_native_player_takes_over(self):
  self.v.status.return_value={'state':'playing','station_stream_id':'yandex:other','current':{'id':'1'}}
  await self.v.accept(3,'дальше',source='spotter')
  self.assertEqual((await self.v.queue.get())[1],('next',''))
 async def test_bare_control_without_apple_queue_not_claimed(self):
  self.v.status.return_value={'state':'stopped','current':None}
  await self.v.accept(3,'дальше',source='spotter')
  self.assertTrue(self.v.queue.empty())
 async def test_same_command_from_two_channels_runs_once(self):
  await self.v.accept(3,'Дальше',source='recognition')
  await self.v.accept(3,'дальше',source='spotter')
  self.assertEqual(self.v.queue.qsize(),1)
 async def test_spotter_stream_ignores_volume_and_handles_caret_prefix(self):
  from unittest.mock import patch
  self.v.config['station_host']='station.test'
  reader=asyncio.StreamReader();reader.feed_data('громче\n^ дальше\n'.encode())
  proc=MagicMock();proc.stdout=reader;proc.returncode=None
  with patch('voice.asyncio.create_subprocess_exec',new_callable=AsyncMock,return_value=proc):
   task=asyncio.create_task(self.v.spotter('/tmp/known_hosts'))
   try:
    self.assertEqual((await asyncio.wait_for(self.v.queue.get(),2))[1],('next',''))
    self.assertTrue(self.v.queue.empty());self.assertEqual(self.v.status.await_count,1)
   finally:
    proc.returncode=0;task.cancel();await asyncio.gather(task,return_exceptions=True)
 async def test_queue_bounded(self):
  for i in range(12):await self.v.accept(3,'включи песню '+str(i))
  self.assertEqual(self.v.queue.qsize(),8)
 async def test_local_spotter_skips_history_and_follows_rotation(self):
  import tempfile
  from pathlib import Path
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/'maind.log';path.write_text('Command phrase spotted: пауза\n')
   task=asyncio.create_task(self.v.local_spotter(path))
   try:
    await asyncio.sleep(.15);self.assertTrue(self.v.queue.empty())
    with path.open('a') as f:f.write('unrelated line\nCommand phrase spotted: дальше\n')
    self.assertEqual((await asyncio.wait_for(self.v.queue.get(),2))[1],('next',''))
    path.rename(path.with_suffix('.old'));path.write_text('Command phrase spotted: пауза\n')
    self.assertEqual((await asyncio.wait_for(self.v.queue.get(),2))[1],('pause',''))
   finally:
    task.cancel();await asyncio.gather(task,return_exceptions=True)
 async def test_reader_handles_fragmented_frame(self):
  done=asyncio.Event()
  async def client(reader,writer):
   payload=frame();wire=struct.pack('<I',len(payload))+payload
   for byte in wire:
    writer.write(bytes([byte]));await writer.drain()
   await done.wait();writer.close();await writer.wait_closed()
  server=await asyncio.start_server(client,'127.0.0.1',0)
  task=asyncio.create_task(self.v.read('127.0.0.1',server.sockets[0].getsockname()[1]))
  try:
   await asyncio.wait_for(self.v.queue.get(),2)
  finally:
   done.set();task.cancel();await asyncio.gather(task,return_exceptions=True)
   server.close();await server.wait_closed()

class AppleSelection(unittest.IsolatedAsyncioTestCase):
 def make(self):
  apple=Apple({},'.','.',None);apple.storefront='us';apple.api=AsyncMock();return apple
 async def test_library_catalog_mapping(self):
  apple=self.make();apple.api.return_value={'data':[
   {'type':'library-songs','id':'i.x','attributes':{'name':'A','playParams':{'catalogId':'123'}}},
   {'type':'library-songs','id':'i.upload','attributes':{'name':'Local'}}]}
  tracks=await apple.selection(mode='library');self.assertEqual([t['id'] for t in tracks],['123'])
 async def test_pagination_and_deduplication(self):
  apple=self.make();song={'type':'songs','id':'1','attributes':{'name':'A'}}
  apple.api.side_effect=[{'data':[song],'next':'/v1/catalog/us/playlists/pl.x/tracks?offset=1'},
                         {'data':[song,dict(song,id='2')]}]
  self.assertEqual([t['id'] for t in await apple.tracks('catalog/us/playlists/pl.x/tracks')],['1','2'])
 async def test_external_pagination_rejected(self):
  apple=self.make();apple.api.return_value={'data':[],'next':'https://evil.example/v1/steal'}
  with self.assertRaises(RuntimeError):await apple.tracks('catalog/us/playlists/pl.x/tracks')
  self.assertEqual(apple.api.await_count,1)
 async def test_exact_artist_uses_top_songs(self):
  apple=self.make();apple.api.side_effect=[{'results':{'artists':{'data':[{'id':'42','attributes':{'name':'Noize MC'}}]}}},
   {'data':[{'type':'songs','id':'1','attributes':{'name':'Song'}}]}]
  self.assertEqual((await apple.selection('noize mc'))[0]['id'],'1')
  self.assertEqual(apple.api.call_args.args[0],'catalog/us/artists/42/view/top-songs')
 async def test_auto_song(self):
  apple=self.make();apple.api.return_value={'results':{'songs':{'data':[{'id':'1','attributes':{'name':'Hello'}}]}}}
  self.assertEqual((await apple.selection('hello'))[0]['title'],'Hello')
 async def test_album_path(self):
  apple=self.make();apple.api.side_effect=[{'results':{'albums':{'data':[{'id':'42'}]}}},{'data':[]}]
  await apple.selection('name','album')
  self.assertEqual(apple.api.call_args.args[0],'catalog/us/albums/42/tracks')
