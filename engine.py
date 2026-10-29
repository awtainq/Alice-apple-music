"""Independent queue, prefetch and natural-end detection for one Station."""
import asyncio
from collections import deque
import hashlib
import hmac
import json
import secrets
import time
from pathlib import Path
from aiohttp import web

from apple import Apple
from station_client import Station,audio_command


class Engine:
    def __init__(self,config,private,cache,http):
        self.config=config;self.apple=Apple(config,private,cache,http)
        self.station=Station(config,http,self.on_state)
        self.current=None;self.upcoming=[];self.history=[];self.seen=set();self.seed=None
        self.state='stopped';self.position=0;self.duration=0;self.confirmed=False;self.generation=0
        self.airplay_active=False
        self.lock=asyncio.Lock();self.prefetch=None;self.prefetched=None;self.transition=None
        self.last_progress=0;self.last_playing=False;self.last_alice='IDLE';self.grace=0
        self.stream_secret=secrets.token_bytes(32);self.fetch_count=0;self.auto_advances=0;self.next_failures=0
        self.last_error=None
        self.station_state={};self.volume_history=deque(maxlen=120);self.voice_recovery=None;self.voice_not_before=0
        self.state_file=Path(private)/'queue.json';self.restored=False;self.last_checkpoint=0
        if self.state_file.exists():
            try:
                saved=json.loads(self.state_file.read_text())
                self.current=saved.get('current');self.upcoming=saved.get('next',[])
                self.history=saved.get('history',[]);self.seen=set(saved.get('seen',[]));self.seed=saved.get('seed')
                self.position=saved.get('position',0);self.duration=(self.current or {}).get('duration',0)
                self.state='paused' if self.current else 'stopped';self.restored=bool(self.current)
            except (ValueError,TypeError,OSError):pass

    def checkpoint(self):
        payload={'current':self.current,'next':self.upcoming,'history':self.history[-30:],
                 'seen':list(self.seen)[-1000:],'seed':self.seed,'position':self.position}
        temporary=self.state_file.with_suffix('.tmp')
        temporary.write_text(json.dumps(payload,ensure_ascii=False));temporary.chmod(0o600)
        temporary.replace(self.state_file);self.last_checkpoint=time.monotonic()

    def status(self):
        return {'state':self.state,'current':self.current,'next':self.upcoming[:5],'airplay_active':self.airplay_active,
                'position':self.position,'duration':self.duration,'fetch_count':self.fetch_count,
                'auto_advances':self.auto_advances,'last_error':self.last_error,
                'station_connected':self.station.ready.is_set(),
                'station_alice':self.station_state.get('aliceState'),
                'station_playing':self.station_state.get('playing'),
                'station_stream_id':self.station_state.get('playerState',{}).get('id'),
                'voice_waiting':bool(self.voice_not_before)}

    def token(self,song_id):return hmac.new(self.stream_secret,song_id.encode(),hashlib.sha256).hexdigest()

    async def serve_audio(self,request):
        song_id=request.match_info['song']
        if not song_id.isdigit() or not hmac.compare_digest(request.match_info['token'],self.token(song_id)):
            raise web.HTTPNotFound()
        path=self.apple.cache/(song_id+'.m4a')
        if not path.exists():raise web.HTTPNotFound()
        if request.method=='GET':
            self.fetch_count+=1
            print(json.dumps({'event':'audio_fetch','song_id':song_id,'bytes':path.stat().st_size}),flush=True)
        return web.FileResponse(path,headers={'Content-Type':'audio/mp4','Cache-Control':'private, no-store'})

    async def refill(self):
        if not self.seed:return
        for _ in range(3):
            tracks=await self.apple.similar(self.seed)
            ids={x['id'] for x in self.upcoming}
            for track in tracks:
                if track['id'] not in self.seen and track['id'] not in ids:
                    self.upcoming.append(track);ids.add(track['id'])
            if self.upcoming:return
            if self.current:self.seed=self.current['id']
        # Do not silently replace unavailable recommendations with unrelated music.
        raise RuntimeError('Apple Music не подобрал новые похожие песни.')

    async def prefetch_next(self,generation):
        try:
            if not self.upcoming:await self.refill()
            if generation!=self.generation or not self.upcoming:return
            track=self.upcoming[0]
            path=await self.apple.prepare(track)
            if generation==self.generation:self.prefetched=(track['id'],path)
        except asyncio.CancelledError:raise
        except Exception as exc:
            self.last_error=str(exc) if isinstance(exc,RuntimeError) else type(exc).__name__
            print('Prefetch:',type(exc).__name__,flush=True)

    def schedule_prefetch(self):
        if self.prefetch and not self.prefetch.done():return
        if self.upcoming and self.prefetched and self.prefetched[0]==self.upcoming[0]['id']:return
        self.prefetch=asyncio.create_task(self.prefetch_next(self.generation))

    async def wait_voice_reply(self):
        if not self.voice_not_before:return
        start=time.monotonic();deadline=start+20;idle_since=None
        while time.monotonic()<deadline:
            response=await self.station.send({'command':'softwareVersion'})
            if response.get('state'):self.station_state=response['state']
            now=time.monotonic()
            if now>=self.voice_not_before and self.station_state.get('aliceState')=='IDLE':
                if idle_since is None:idle_since=now
                if now-idle_since>=.8:
                    self.voice_not_before=0
                    print(json.dumps({'event':'voice_reply_finished','wait_seconds':round(now-start,2)}),flush=True)
                    return
            else:idle_since=None
            await asyncio.sleep(.2)
        raise RuntimeError('Колонка ещё слушает. Повторите команду через несколько секунд.')

    async def play_track(self,track):
        path=await self.apple.prepare(track)
        if self.airplay_active:raise RuntimeError('Сначала отключите AirPlay на телефоне.')
        await self.wait_voice_reply()
        if self.airplay_active:raise RuntimeError('Сначала отключите AirPlay на телефоне.')
        self.generation+=1;generation=self.generation
        self.current=track;self.seen.add(track['id']);self.position=0;self.last_progress=0
        self.duration=track.get('duration',0);self.confirmed=False;self.state='starting';self.grace=time.monotonic()+5
        stream_id='alice-music-light:'+track['id']+':'+str(generation)
        self.stream_id=stream_id
        url=f'http://{self.config["bind_host"]}:{self.config["port"]}/audio/{self.token(track["id"])}/{track["id"]}.m4a'
        self.fetch_before=self.fetch_count
        await self.station.send(audio_command(url,track,stream_id))
        self.state='playing';self.last_error=None;self.restored=False;self.checkpoint()
        self.schedule_prefetch()
        self.apple.trim_cache(keep=[track['id']]+[x['id'] for x in self.upcoming[:self.config.get('cache_keep_next',2)]])
        print(json.dumps({'event':'play','track':track},ensure_ascii=False),flush=True)

    async def play_query(self,query):
        async with self.lock:
            if self.prefetch and not self.prefetch.done():self.prefetch.cancel()
            self.generation+=1;self.prefetched=None
            tracks=await self.apple.search(query)
            if not tracks:raise RuntimeError('В Apple Music ничего не найдено. Уточните песню и исполнителя.')
            self.history=[];self.upcoming=[];self.seen=set();self.seed=tracks[0]['id']
            await self.play_track(tracks[0])
            return 'Включаю '+tracks[0]['title']+', '+tracks[0]['artist']+'. Дальше будут похожие песни.'

    async def play_selection(self,query='',mode='auto'):
        async with self.lock:
            # Preserve the old queue if search fails or returns no playable items.
            tracks=await self.apple.selection(query,mode)
            if not tracks:raise RuntimeError('В Apple Music ничего не найдено. Уточните запрос.')
            if self.prefetch and not self.prefetch.done():
                self.prefetch.cancel()
                await asyncio.gather(self.prefetch,return_exceptions=True)
            self.generation+=1;self.prefetched=None
            self.history=[];self.upcoming=tracks[1:];self.seen=set();self.seed=tracks[0]['id']
            await self.play_track(tracks[0])
            return 'Включаю.'

    async def next(self,automatic=False):
        async with self.lock:
            old=self.current
            if self.prefetch and not self.prefetch.done():
                try:await asyncio.wait_for(asyncio.shield(self.prefetch),30)
                except TimeoutError:pass
            if not self.upcoming:await self.refill()
            if not self.upcoming:raise RuntimeError('Похожих песен пока нет.')
            track=self.upcoming[0];self.prefetched=None
            await self.play_track(track)
            self.upcoming.pop(0)
            if old:self.history.append(old)
            self.checkpoint()
            if automatic:self.auto_advances+=1
            return 'Включаю '+track['title']+', '+track['artist']+'.'

    async def previous(self):
        async with self.lock:
            if not self.history:return 'Предыдущей песни пока нет.'
            if self.current:self.upcoming.insert(0,self.current)
            track=self.history.pop();await self.play_track(track)
            return 'Включаю '+track['title']+'.'

    async def pause(self):
        async with self.lock:
            self.generation+=1
            if self.voice_recovery and not self.voice_recovery.done():self.voice_recovery.cancel()
            self.state='paused';self.grace=time.monotonic()+2
            await self.station.send({'command':'stop'});self.checkpoint()
            return 'Пауза.'

    async def resume(self):
        async with self.lock:
            if not self.current:return 'Сначала назовите песню.'
            response=await self.station.send({'command':'softwareVersion'})
            if isinstance(response.get('state'),dict):self.station_state=response['state']
            if self.station_state.get('aliceState') not in (None,'','IDLE'):
                self.voice_not_before=max(self.voice_not_before,time.monotonic())
            await self.wait_voice_reply()
            if self.restored or not self.station_state.get('playing'):
                # Native play may acknowledge a paused bypass stream without
                # restarting it. Reopen the same AAC and restore its position.
                position=self.position;await self.play_track(self.current)
                if position>1:
                    await asyncio.sleep(.5)
                    await self.station.send({'command':'rewind','position':position})
                    self.position=position;self.last_progress=position;self.checkpoint()
            else:
                self.state='playing';self.grace=time.monotonic()+4
            return 'Продолжаю.'

    async def seek(self,position):
        async with self.lock:
            self.position=max(0,min(float(position),self.duration));self.last_progress=self.position
            self.grace=time.monotonic()+2
            await self.station.send({'command':'rewind','position':self.position})
            return 'Перематываю.'

    async def recover_after_voice(self,generation,stream_id,volume,timeout=18,resume_if_stopped=False):
        # audio_play can start while Alice is ducking output for her reply.
        # Once that reply has finished, restore only the same requested stream.
        try:
            await asyncio.sleep(2)
            deadline=time.monotonic()+timeout;idle_since=None
            while time.monotonic()<deadline:
                if generation!=self.generation:return
                state=self.station_state
                if state.get('aliceState')=='IDLE':
                    if idle_since is None:idle_since=time.monotonic()
                    if time.monotonic()-idle_since>=.8:break
                else:idle_since=None
                await asyncio.sleep(.2)
            else:return
            if generation!=self.generation:return
            response=await self.station.send({'command':'softwareVersion'})
            if isinstance(response.get('state'),dict):self.station_state=response['state']
            state=self.station_state
            if state.get('playerState',{}).get('id')!=stream_id:return
            if resume_if_stopped and not state.get('playing'):
                await self.resume()
                return
            if generation!=self.generation:return
            current_volume=state.get('volume')
            # Undo the firmware's exact half-volume duck, preserving mute and
            # unrelated user volume changes. Never choose a new default volume.
            if (isinstance(volume,(float,int)) and volume>0
                and isinstance(current_volume,(float,int))
                and current_volume<volume and abs(current_volume-volume/2)<=.015):
                await self.station.send({'command':'setVolume','volume':volume})
            self.state='playing';self.grace=time.monotonic()+3
            print(json.dumps({'event':'voice_playback_restored','song_id':(self.current or {}).get('id')}),flush=True)
        except asyncio.CancelledError:raise
        except Exception as exc:print('Voice playback recovery:',type(exc).__name__,flush=True)

    async def command(self,action,query='',voice=False):
        volume=None
        start_actions={'play','auto','artist','album','playlist','library','next','previous','resume','similar'}
        recover_current=(action!='now' or self.state=='playing')
        if voice and action in start_actions|{'now'}:
            cutoff=time.monotonic()-3
            earlier=[v for t,v in self.volume_history if t<=cutoff]
            volume=earlier[-1] if earlier else self.station_state.get('volume')
        generation_before=self.generation
        if voice and action in start_actions:
            self.voice_not_before=time.monotonic()+float(self.config.get('voice_start_delay',3))
        try:result=await self._command(action,query)
        finally:self.voice_not_before=0
        if voice and recover_current and action in start_actions|{'now'} and self.current and (action in {'resume','now'} or self.generation!=generation_before):
            if self.voice_recovery and not self.voice_recovery.done():self.voice_recovery.cancel()
            self.voice_recovery=asyncio.create_task(self.recover_after_voice(self.generation,getattr(self,'stream_id',None),volume,45 if action=='now' else 18,action=='now'))
        return result

    async def _command(self,action,query=''):
        if self.airplay_active and action not in ('pause','stop'):
            raise RuntimeError('Сначала отключите AirPlay на телефоне.')
        if action=='play':return await self.play_query(query)
        if action in {'auto','artist','album','playlist','library'}:return await self.play_selection(query,action)
        if action=='next':return await self.next()
        if action=='previous':return await self.previous()
        if action in ('pause','stop'):return await self.pause()
        if action=='resume':return await self.resume()
        if action=='similar':
            if not self.current:return 'Сначала назовите песню.'
            self.seed=self.current['id'];self.upcoming=[];self.seen={self.seed};self.prefetched=None
            return await self.next()
        if action=='now':
            return (self.current['title']+', '+self.current['artist']) if self.current else 'Музыка ещё не включена.'
        if action=='seek':return await self.seek(float(query))
        raise ValueError('Unknown action')

    async def auto_next(self):
        try:await self.next(automatic=True);self.next_failures=0
        except Exception as exc:
            self.state='paused';self.last_error=str(exc) if isinstance(exc,RuntimeError) else type(exc).__name__
            print('Autoplay:',type(exc).__name__,flush=True)

    async def on_state(self,state):
        self.station_state=state
        if state.get('aliceState')=='IDLE' and isinstance(state.get('volume'),(float,int)):
            self.volume_history.append((time.monotonic(),state['volume']))
        if not self.current:return
        player=state.get('playerState',{});playing=bool(state.get('playing'))
        alice=state.get('aliceState','IDLE');progress=float(player.get('progress',0) or 0)
        if playing:
            self.confirmed=True;self.position=progress;self.last_progress=progress
            if self.state=='starting':self.state='playing'
        if (self.state=='playing' and self.confirmed and not playing and self.last_playing
            and alice=='IDLE' and self.last_alice=='IDLE' and time.monotonic()>self.grace):
            if self.last_progress>=self.duration-3 and self.fetch_count>self.fetch_before:
                if not self.transition or self.transition.done():self.transition=asyncio.create_task(self.auto_next())
            else:self.state='paused'
        self.last_playing=playing;self.last_alice=alice

    async def watch(self):
        while True:
            await asyncio.sleep(2)
            if self.state=='playing' and self.current:
                self.schedule_prefetch()
                if time.monotonic()-self.last_checkpoint>self.config.get('checkpoint_seconds',10):self.checkpoint()
