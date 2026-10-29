"""Read Alice's local recognition IPC through a private SSH tunnel.

Wire format documented by Yandex Station Extended / Mark Pisarenko (CC BY 4.0):
https://github.com/Marker284/yandex-station-extended
Independent Python implementation; no audio or arbitrary utterances are logged.
"""
import argparse
import asyncio
import json
import os
import re
import struct
import time
from pathlib import Path

import aiohttp
from station_client import varint

MAX_FRAME = 600000


def byte_field(number, value):
    return varint(number*8+2)+varint(len(value))+value


def rpc_frame(field, payload):
    remote=byte_field(7,b'AliceCapability')+byte_field(field,payload)
    envelope=byte_field(252,remote)+varint(253*8)+varint(210)
    return struct.pack('<I',len(envelope))+envelope


def cancel_frame():
    # Stop listening as well as the outstanding reply; cancelling a request alone
    # can leave Alice waiting for another utterance. Musical phrases only.
    return b''.join(rpc_frame(8,bytes([8,method,56,1])) for method in (3,6,14))


async def vocalize(text, host='127.0.0.1', port=19875):
    """Speak through the local Alice capability, without opening a skill.

    Frame documented by YSE / rison1337 and v3rt3p/quasar-protos.
    TTS event confirmation is best effort and is not a proof of audible output.
    """
    reader,writer=await asyncio.open_connection(host,port)
    try:
        subscribe=rpc_frame(20,byte_field(1,b'AliceCapability'))
        directive=byte_field(1,text[:2000].encode())+b'\x10\x02'
        writer.write(subscribe+rpc_frame(8,b'\x08\x12\x18\x01'+byte_field(13,directive)))
        await writer.drain();started=False
        async with asyncio.timeout(12):
            while True:
                size=struct.unpack('<I',await reader.readexactly(4))[0]
                if not 0<size<=MAX_FRAME:raise ValueError('Invalid frame')
                outer=dict(fields(await reader.readexactly(size)))
                if outer.get(253)!=3:continue
                remote=dict(fields(outer.get(252,b'')))
                if 18 not in remote:continue
                event=dict(fields(remote[18])).get(1,0)
                if event==6:started=True
                elif event==1 and started:return True
        return False
    except TimeoutError:
        return False
    finally:
        writer.close();await writer.wait_closed()


def fields(data):
    """Decode only protobuf wire types needed by recognition events."""
    pos = 0
    def varint():
        nonlocal pos
        value = 0
        for shift in range(0, 70, 7):
            if pos >= len(data):
                raise ValueError('Truncated varint')
            byte = data[pos]; pos += 1
            if shift == 63 and byte > 1:
                raise ValueError('Oversized varint')
            value |= (byte & 127) << shift
            if byte < 128:
                return value
        raise ValueError('Oversized varint')
    while pos < len(data):
        tag = varint(); number, wire = tag >> 3, tag & 7
        if not number:
            raise ValueError('Invalid field')
        if wire == 0:
            yield number, varint()
        elif wire in (1, 2, 5):
            size = varint() if wire == 2 else (8 if wire == 1 else 4)
            if size > len(data) - pos:
                raise ValueError('Truncated field')
            value = data[pos:pos+size]; pos += size
            yield number, value
        else:
            raise ValueError('Unsupported wire type')


def recognition(frame):
    try:
        outer = dict(fields(frame))
        if outer.get(253) != 3 or not isinstance(outer.get(252), bytes):
            return None
        event = dict(fields(outer[252])); state = event.get(1, 0)
        text = event.get(2, b'')
        if not isinstance(state, int) or not isinstance(text, bytes):
            return None
        return state, text.decode('utf-8') if state == 3 else ''
    except (ValueError, UnicodeDecodeError, TypeError):
        return None


def parse_phrase(text, recent=False):
    """Conservative local routing: other Alice features keep their commands."""
    text = re.sub(r'\s+', ' ', text).strip(' .!?').casefold().replace('ё', 'е')
    text = re.sub(r'^алиса[, ]+', '', text)
    text = re.sub(r'[, ]+пожалуйста$', '', text)
    # Skill replies and scenario-generated sendText must never recurse.
    if not text or len(text) > 500 or re.search(r'\bнавык\w*\b', text):
        return None
    controls = {
        'next': ('дальше', 'следующая', 'следующая песня', 'следующий трек', 'музыку дальше'),
        'previous': ('назад', 'предыдущая', 'предыдущая песня', 'предыдущий трек', 'музыку назад'),
        'pause': ('пауза', 'стоп', 'хватит', 'выключи музыку', 'останови музыку', 'поставь на паузу', 'поставь музыку на паузу'),
        'resume': ('играй', 'продолжай', 'продолжи', 'продолжи музыку', 'продолжай музыку'),
        'similar': ('включи похожие', 'похожие песни', 'включи похожую музыку'),
        'announce_now': ('что играет', 'что сейчас играет', 'что за песня', 'что за песня сейчас играет', 'какая песня играет', 'какая песня сейчас играет', 'что за трек', 'что за трек сейчас играет', 'какая музыка играет', 'кто поет'),
    }
    for action, phrases in controls.items():
        if text in phrases:
            return (action, '') if recent else None
    match = re.fullmatch(r'(?:поставь|включи|играй|сыграй) (.+)', text)
    if not match:
        return None
    query = re.sub(r' (?:в|на|из) (?:эпл мьюзик|эпл мюзик|apple music)$', '', match[1])
    if query in ('музыку', 'музыка', 'мою музыку', 'мои песни', 'медиатеку', 'любимую музыку', 'любимые песни'):
        return 'library', ''
    for prefix, action in [('альбом ', 'album'), ('плейлист ', 'playlist'), ('подборку ', 'playlist'),
                           ('исполнителя ', 'artist'), ('песни исполнителя ', 'artist'),
                           ('песню ', 'play'), ('трек ', 'play'), ('композицию ', 'play'), ('музыку ', 'auto')]:
        if query.startswith(prefix):
            query = query[len(prefix):].strip()
            return (action, query) if query else None
    if re.search(r'\b(?:свет\w*|ламп\w*|люстр\w*|телевизор\w*|розетк\w*|пылесос\w*|'
                 r'чайник\w*|таймер\w*|будильник\w*|напомин\w*|кондиционер\w*|'
                 r'радио|новости|сказку|сказки|блютуз|bluetooth|громкость|тише|громче)\b', query):
        return None
    return 'auto', query


class Voice:
    def __init__(self, config, session):
        self.config, self.http = config, session
        self.queue = asyncio.Queue(maxsize=8)
        self.last_text = ''; self.last_at = 0; self.active_at = 0
        self.ipc_writer = None
        self.last_routed = None

    async def status(self):
        async with self.http.get(self.url, headers=self.headers) as response:
            response.raise_for_status()
            return await response.json()

    @property
    def url(self):
        return f'http://{self.config["bind_host"]}:{self.config["port"]}/api'

    @property
    def headers(self):
        return {'Authorization': 'Bearer '+self.config['agent_token']}

    async def accept(self, state, text, source='recognition'):
        if state == 1:
            self.last_text = ''  # A fresh spoken repeat is a new command.
        if state != 3 or not text:
            return
        now = time.monotonic()
        if text == self.last_text and now-self.last_at < 3:
            return
        self.last_text, self.last_at = text, now
        # Do not query the player or forward non-musical speech.
        candidate=parse_phrase(text, True)
        if candidate is None:
            return
        status = await self.status()
        if status.get('airplay_active'):
            # Luna routes transport to the live sender. Do not also advance or
            # resume our saved queue, nor cancel Luna's claimed voice request.
            return
        recent = (self.active_at>0 and now-self.active_at < 3600 or
                  str(status.get('station_stream_id', '')).startswith('alice-music-light:') or
                  candidate[0]=='announce_now' and bool(status.get('current')) or
                  source=='spotter' and bool(status.get('current')))
        command = parse_phrase(text, recent)
        if command and not self.queue.full():
            # A phrase may arrive both from recognition and the local spotter.
            if (self.last_routed and self.last_routed[0]==command and
                self.last_routed[1]!=source and now-self.last_routed[2]<2):
                return
            self.last_routed=(command,source,now)
            if self.ipc_writer:
                self.ipc_writer.write(cancel_frame())
                await self.ipc_writer.drain()
            self.queue.put_nowait((now, command))

    async def spotter(self, known_hosts):
        """Read only command-spotter lines, filtered on the Station before SSH.

        Bare control words do not appear as ordinary recognition events. tail -F
        follows firmware log rotation; no log history or general speech is read.
        """
        remote=("tail -n 0 -F /data/quasar/daemons_logs/maind.log 2>/dev/null | "
                "awk -F 'Command phrase spotted: ' 'NF>1 {print $2; fflush()}'")
        args=['ssh','-T','-p',str(self.config.get('station_ssh_port',2222)),
              '-o','BatchMode=yes','-o','StrictHostKeyChecking=yes',
              '-o','UserKnownHostsFile='+str(known_hosts),'-o','ConnectTimeout=6',
              '-o','ServerAliveInterval=15','-o','ServerAliveCountMax=3',
              'root@'+self.config['station_host'],remote]
        ssh_config=Path(known_hosts).with_name('voice_ssh_config')
        if ssh_config.exists():
            # Reuse the tunnel's host/key settings without binding its port again.
            args=['ssh','-T','-F',str(ssh_config),'-o','ClearAllForwardings=yes',
                  '-o','BatchMode=yes','-o','StrictHostKeyChecking=yes',
                  '-o','ConnectTimeout=6','-o','ServerAliveInterval=15',
                  '-o','ServerAliveCountMax=3','station-music',remote]
        allowed={'дальше','назад','пауза','стоп','хватит','играй','продолжай'}
        while True:
            proc=None
            try:
                proc=await asyncio.create_subprocess_exec(*args,stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.DEVNULL,limit=4096)
                print('Bare command listener started.',flush=True)
                while line:=await proc.stdout.readline():
                    word=re.sub(r'^\^\s*','',line.decode(errors='replace').strip()).casefold()
                    if word not in allowed:continue
                    try:await self.accept(3,word,source='spotter')
                    except (aiohttp.ClientError,TimeoutError):
                        print('Player unavailable for bare command.',flush=True)
            except OSError as exc:
                print('Bare command listener:',type(exc).__name__,flush=True)
            finally:
                if proc and proc.returncode is None:
                    proc.terminate()
                    try:await asyncio.wait_for(proc.wait(),3)
                    except TimeoutError:proc.kill();await proc.wait()
            await asyncio.sleep(3)

    async def local_spotter(self, path):
        """Follow the Station's local spotter log without SSH or child processes."""
        path=Path(path);handle=None;identity=None;first=True
        allowed={'дальше','назад','пауза','стоп','хватит','играй','продолжай'}
        try:
            while True:
                try:
                    stat=path.stat();current=(stat.st_dev,stat.st_ino)
                    if handle is None or current!=identity:
                        if handle:handle.close()
                        handle=path.open(encoding='utf-8',errors='replace');identity=current
                        if first:handle.seek(0,os.SEEK_END);first=False
                    elif stat.st_size<handle.tell():handle.seek(0)
                    for _ in range(100):
                        line=handle.readline(8192)
                        if not line:break
                        if 'Command phrase spotted: ' not in line:continue
                        word=re.sub(r'^\^\s*','',line.split('Command phrase spotted: ',1)[1].strip()).casefold()
                        if word not in allowed:continue
                        try:await self.accept(3,word,source='spotter')
                        except (aiohttp.ClientError,TimeoutError):
                            print('Player unavailable for bare command.',flush=True)
                except FileNotFoundError:pass
                await asyncio.sleep(.1)
        finally:
            if handle:handle.close()

    async def execute(self):
        while True:
            created, (action, query) = await self.queue.get()
            try:
                if time.monotonic()-created > 30:
                    continue
                print(json.dumps({'event': 'direct_voice_command', 'action': action}), flush=True)
                async with self.http.post(self.url, headers=self.headers,
                    json={'action': action, 'query': query, 'voice': True},
                    timeout=aiohttp.ClientTimeout(total=100)) as response:
                    response.raise_for_status()
                    self.active_at = time.monotonic()
                print(json.dumps({'event': 'direct_voice_finished', 'action': action}), flush=True)
            except Exception as exc:
                # URLs, query text, credentials and response bodies stay out of logs.
                print('Direct voice command:', type(exc).__name__, flush=True)
            finally:
                self.queue.task_done()

    async def read(self, host, port):
        while True:
            writer = None
            try:
                reader, writer = await asyncio.open_connection(host, port)
                self.ipc_writer = writer
                print('Recognition channel connected.', flush=True)
                while True:
                    size = struct.unpack('<I', await reader.readexactly(4))[0]
                    if not 0 < size <= MAX_FRAME:
                        raise ValueError('Invalid frame size')
                    event = recognition(await reader.readexactly(size))
                    if event:
                        try:
                            await self.accept(*event)
                        except (aiohttp.ClientError, TimeoutError):
                            print('Player unavailable.', flush=True)
            except (OSError, asyncio.IncompleteReadError, ValueError) as exc:
                print('Recognition reconnect:', type(exc).__name__, flush=True)
            finally:
                self.ipc_writer = None
                if writer:
                    writer.close()
                    await writer.wait_closed()
            await asyncio.sleep(3)


async def main(args):
    config = json.loads(Path(args.config).read_text())
    async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=5)) as http:
        voice = Voice(config, http)
        tasks=[voice.read(args.host,args.port),voice.execute()]
        if args.spotter_ssh:tasks.append(voice.spotter(Path(args.config).resolve().parent/'station_known_hosts'))
        await asyncio.gather(*tasks)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--config', required=True)
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=19875)
    parser.add_argument('--spotter-ssh',action='store_true',help='Also route bare local music controls over SSH')
    asyncio.run(main(parser.parse_args()))
