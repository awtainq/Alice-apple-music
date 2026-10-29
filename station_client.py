"""Local Glagol control. Protocol: AlexxIT/YandexStation (MIT)."""
import asyncio
import base64
import json
import socket
import time
import uuid

import aiohttp


def varint(n):
    out=bytearray()
    while n>127:out.append((n&127)|128);n>>=7
    return bytes(out)+bytes([n])


def audio_command(url, track, stream_id):
    data={'stream':{'url':url,'format':'MP3','type':'Track','id':stream_id},
          'metadata':{'title':track['title'],'subtitle':track.get('artist','')}}
    name=b'audio_play';body=json.dumps(data,ensure_ascii=False).encode()
    wire=b'\x0a'+varint(len(name))+name+b'\x12'+varint(len(body))+body
    return {'command':'externalCommandBypass','data':base64.b64encode(wire).decode()}


class Station:
    def __init__(self, config, session, on_state):
        self.config,self.http,self.on_state=config,session,on_state
        self.ws=None;self.token=None;self.waiters={};self.ready=asyncio.Event();self.closed=False

    async def device_token(self):
        c=self.config
        for attempt in range(2):
            async with self.http.get('https://quasar.yandex.net/glagol/token',
                headers={'Authorization':'OAuth '+c['yandex_music_token']},
                params={'device_id':c['station_device_id'],'platform':c['station_platform']}) as r:
                if r.status==200:return (await r.json(content_type=None))['token']
                if r.status not in (401,403):r.raise_for_status()
            if attempt==0:
                async with self.http.post('https://oauth.mobile.yandex.net/1/token',data={
                    'client_secret':'53bc75238f0c4d08a118e51fe9203300','client_id':'23cabbbdc6cd418abb4b39c32c41195d',
                    'grant_type':'x-token','access_token':c['yandex_x_token']}) as r:
                    r.raise_for_status();c['yandex_music_token']=(await r.json())['access_token']
        raise RuntimeError('Нужно заново подключить Яндекс-аккаунт.')

    async def run(self):
        delay=1
        while not self.closed:
            try:
                addresses=await asyncio.get_running_loop().getaddrinfo(self.config['station_host'],
                    self.config['station_port'],family=socket.AF_INET,type=socket.SOCK_STREAM)
                host=addresses[0][4][0];self.token=await self.device_token()
                async with self.http.ws_connect(f'wss://{host}:{self.config["station_port"]}',ssl=False,
                                                heartbeat=30) as ws:
                    self.ws=ws;self.ready.set();delay=1
                    await ws.send_json({'conversationToken':self.token,'id':str(uuid.uuid4()),
                                        'payload':{'command':'softwareVersion'},'sentTime':int(time.time()*1000)})
                    async for msg in ws:
                        if msg.type!=aiohttp.WSMsgType.TEXT:continue
                        data=json.loads(msg.data);key=data.get('requestId')
                        if key in self.waiters and not self.waiters[key].done():self.waiters[key].set_result(data)
                        if data.get('state'):await self.on_state(data['state'])
            except asyncio.CancelledError:raise
            except Exception as exc:
                print('Station connection:',type(exc).__name__,str(getattr(exc,'os_error',''))[:180],flush=True)
            finally:
                self.ready.clear();self.ws=None
                for future in self.waiters.values():
                    if not future.done():future.set_exception(ConnectionError('Station disconnected'))
                self.waiters.clear()
            await asyncio.sleep(delay);delay=min(delay*2,30)

    async def send(self, payload):
        await asyncio.wait_for(self.ready.wait(),15)
        key=str(uuid.uuid4());future=asyncio.get_running_loop().create_future();self.waiters[key]=future
        try:
            await self.ws.send_json({'conversationToken':self.token,'id':key,'payload':payload,
                                     'sentTime':int(time.time()*1000)})
            response=await asyncio.wait_for(future,5)
            if response.get('status')!='SUCCESS':raise RuntimeError('Станция отклонила команду.')
            return response
        finally:self.waiters.pop(key,None)
