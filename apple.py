"""Small Apple Music client. Streaming protocol adapted from Music Assistant (Apache-2.0).
Uses only the subscription credentials and decoder assets already authorized locally.
"""
import asyncio
import base64
import json
import os
import re
import shutil
import time
from pathlib import Path
from urllib.parse import urljoin, urlsplit

import aiohttp
from pywidevine import Cdm, Device, DeviceTypes, PSSH
from pywidevine.license_protocol_pb2 import WidevinePsshData


class Apple:
    def __init__(self, config, private, cache, session):
        self.config, self.private, self.cache, self.http = config, Path(private), Path(cache), session
        self.storefront = None
        self.lock = asyncio.Lock()
        self.last_request = 0
        self.prepared = {}
        self.track_locks = {}

    async def api(self, endpoint, method='GET', params=None):
        headers = {'Authorization': 'Bearer '+self.config['apple_app_token'],
                   'Music-User-Token': self.config['apple_user_token']}
        for attempt in range(5):
            async with self.lock:
                await asyncio.sleep(max(0, .3-(time.monotonic()-self.last_request)))
                self.last_request = time.monotonic()
                async with self.http.request(method, 'https://api.music.apple.com/v1/'+endpoint,
                                             headers=headers, params=params, json={} if method=='POST' else None) as r:
                    if r.status not in (429,500,502,503,504):
                        if r.status in (401,403):raise RuntimeError('Нужно заново привязать Apple Music.')
                        r.raise_for_status()
                        return await r.json()
            await asyncio.sleep(min(2**attempt,8))
        raise RuntimeError('Apple Music временно ограничил запросы.')

    async def initialize(self):
        self.storefront = (await self.api('me/storefront'))['data'][0]['id']

    @staticmethod
    def song(raw):
        a=raw.get('attributes',{})
        return {'id':raw['id'],'title':a.get('name','Без названия'), 'artist':a.get('artistName',''),
                'duration':a.get('durationInMillis',0)/1000}

    async def search(self, query):
        data=await self.api(f'catalog/{self.storefront}/search',params={'term':query,'types':'songs','limit':10})
        return [self.song(x) for x in data.get('results',{}).get('songs',{}).get('data',[])]

    @classmethod
    def playable(cls, raw):
        if raw.get('type')=='library-songs':
            catalog=raw.get('attributes',{}).get('playParams',{}).get('catalogId')
            if not catalog:return None  # Uploaded files have no catalog streaming asset.
            raw=dict(raw,id=str(catalog))
        if raw.get('type') not in ('songs','library-songs') or not str(raw.get('id','')).isdigit():return None
        return cls.song(raw)

    async def tracks(self, endpoint, params=None, maximum=500):
        result=[];seen=set();visited=set()
        while endpoint and len(result)<maximum:
            if endpoint in visited:break
            visited.add(endpoint)
            data=await self.api(endpoint,params=params);params=None
            for raw in data.get('data',[]):
                track=self.playable(raw)
                if track and track['id'] not in seen:
                    result.append(track);seen.add(track['id'])
                    if len(result)>=maximum:break
            link=data.get('next')
            if not link:break
            parsed=urlsplit(link)
            if parsed.netloc or not parsed.path.startswith('/v1/'):
                raise RuntimeError('Apple Music вернул неподдерживаемую страницу списка.')
            endpoint=link[len('/v1/'):]
        return result

    async def selection(self, query='', mode='auto'):
        if mode=='library':
            return await self.tracks('me/library/songs',{'limit':100})
        types={'auto':'songs,artists','artist':'artists','album':'albums','playlist':'playlists'}
        if mode not in types:raise ValueError('Invalid music selection')
        data=await self.api(f'catalog/{self.storefront}/search',params={'term':query,'types':types[mode],'limit':10})
        results=data.get('results',{})
        songs=results.get('songs',{}).get('data',[])
        artists=results.get('artists',{}).get('data',[])
        if mode=='auto':
            normalize=lambda v:re.sub(r'[^\w]+',' ',v.casefold().replace('ё','е')).strip()
            if artists and normalize(query)==normalize(artists[0].get('attributes',{}).get('name','')):
                mode='artist'
            elif songs:return [self.song(songs[0])]
            else:return []
        resource={'artist':'artists','album':'albums','playlist':'playlists'}[mode]
        choices=results.get(resource,{}).get('data',[])
        if not choices:return []
        identifier=choices[0]['id']
        # IDs are supplied by Apple; do not allow them to change the request path.
        if not re.fullmatch(r'[\w.\-]+',identifier):raise ValueError('Invalid catalog resource')
        suffix='view/top-songs' if mode=='artist' else 'tracks'
        return await self.tracks(f'catalog/{self.storefront}/{resource}/{identifier}/{suffix}')

    async def similar(self, seed):
        data=await self.api('me/stations/next-tracks/ra.'+seed, 'POST', {'include':'artists'})
        return [self.song(x) for x in data.get('data',[]) if x.get('id') and x.get('type')=='songs']

    @property
    def playback_headers(self):
        return {'authorization':'Bearer '+self.config['apple_app_token'],
                'media-user-token':self.config['apple_user_token'], 'origin':'https://music.apple.com',
                'referer':'https://music.apple.com/', 'content-type':'application/json;charset=utf-8',
                'user-agent':'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/130 Safari/537.36'}

    async def stream_info(self, song_id):
        async with self.http.post('https://play.music.apple.com/WebObjects/MZPlay.woa/wa/webPlayback',
                                  headers=self.playback_headers,json={'salableAdamId':song_id}) as r:
            r.raise_for_status(); data=await r.json()
        if data.get('failureType'):raise RuntimeError('Apple Music не разрешил воспроизведение этого трека.')
        metadata=data['songList'][0]
        asset=next((x for x in metadata.get('assets',[]) if x.get('flavor')=='28:ctrp256'),None)
        if not asset:raise RuntimeError('Для этой песни нет поддерживаемого аудиопотока.')
        url=asset['URL'];uri=None
        for _ in range(4):
            async with self.http.get(url) as r:r.raise_for_status(); text=await r.text()
            match=re.search(r'#EXT-X-KEY:.*?URI="([^"]+)"',text)
            if match:uri=match[1]
            lines=[x.strip() for x in text.splitlines() if x.strip() and not x.startswith('#')]
            if not lines:raise RuntimeError('Пустой плейлист Apple Music.')
            url=urljoin(url,lines[0])
            if uri:break
        if not uri:raise RuntimeError('Не удалось подготовить поток Apple Music.')
        key_id=base64.b64decode(uri.split(',')[1])
        pssh_data=WidevinePsshData();pssh_data.algorithm=WidevinePsshData.AESCTR;pssh_data.key_ids.append(key_id)
        pssh=PSSH.new(system_id=PSSH.SystemId.Widevine,
                      init_data=base64.b64encode(pssh_data.SerializeToString()).decode())
        device=Device(client_id=(self.private/'client_id.bin').read_bytes(),
                      private_key=(self.private/'private_key.pem').read_bytes(),
                      type_=DeviceTypes.ANDROID,security_level=3,flags={})
        cdm=Cdm.from_device(device);sid=cdm.open()
        try:
            challenge=base64.b64encode(cdm.get_license_challenge(sid,pssh)).decode()
            async with self.http.post(metadata['hls-key-server-url'],headers=self.playback_headers,
                                      json={'challenge':challenge,'key-system':'com.widevine.alpha','uri':uri,
                                            'adamId':song_id,'isLibrary':False,'user-initiated':True}) as r:
                r.raise_for_status(); response=await r.json()
            if not response.get('license'):raise RuntimeError('Apple Music не выдал лицензию подписки.')
            cdm.parse_license(sid,response['license'])
            key=next((k.key.hex() for k in cdm.get_keys(sid) if k.type=='CONTENT'),None)
            if not key:raise RuntimeError('Не удалось открыть аудиопоток подписки.')
            return url,key
        finally:cdm.close(sid)

    async def prepare(self, track):
        lock=self.track_locks.setdefault(track['id'],asyncio.Lock())
        async with lock:return await self._prepare(track)

    async def _prepare(self, track):
        song_id=track['id']
        if not re.fullmatch(r'\d+',song_id):raise ValueError('Invalid catalog ID')
        destination=self.cache/(song_id+'.m4a')
        if destination.exists() and destination.stat().st_size>1000:
            destination.touch();return destination
        url,key=await self.stream_info(song_id)
        encrypted=self.cache/(song_id+'.download')
        partial=self.cache/(song_id+'.part.m4a')
        try:
            size=0
            async with self.http.get(url) as r:
                r.raise_for_status()
                with encrypted.open('wb') as f:
                    async for data in r.content.iter_chunked(65536):
                        size+=len(data)
                        if size>self.config.get('max_download_bytes',80*1024*1024):raise RuntimeError('Аудиофайл слишком большой для локального буфера.')
                        f.write(data)
            # Copy the original AAC; no PCM conversion, loudness analysis or ML processing.
            proc=await asyncio.create_subprocess_exec(self.config.get('ffmpeg') or shutil.which('ffmpeg') or '/opt/homebrew/bin/ffmpeg','-hide_banner','-loglevel','error',
                '-y','-decryption_key',key,'-i',str(encrypted),'-vn','-c:a','copy','-movflags','+faststart',str(partial),
                stdout=asyncio.subprocess.DEVNULL,stderr=asyncio.subprocess.DEVNULL)
            try:
                await asyncio.wait_for(proc.wait(),60)
                if proc.returncode:raise RuntimeError('Не удалось подготовить AAC для колонки.')
            finally:
                if proc.returncode is None:proc.kill();await proc.wait()
            partial.replace(destination)
            return destination
        finally:
            encrypted.unlink(missing_ok=True);partial.unlink(missing_ok=True)

    def trim_cache(self, keep=()):
        files=sorted(self.cache.glob('*.m4a'),key=lambda p:p.stat().st_mtime,reverse=True)
        total=0
        for i,p in enumerate(files):
            total+=p.stat().st_size
            if p.stem not in keep and (i>=self.config.get('cache_files',4) or total>self.config.get('cache_bytes',96*1024*1024)):p.unlink(missing_ok=True)
