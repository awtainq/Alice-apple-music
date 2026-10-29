#!/usr/bin/env python3
"""Install Alice Apple Music over OpenSSH; only Python's standard library is needed."""
import argparse
import hashlib
import io
import json
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from urllib.request import urlopen

REPOSITORY = 'awtainq/Alice-apple-music'
RELEASE = f'https://github.com/{REPOSITORY}/releases/latest/download/'
REQUIRED = ('config.json', 'client_id.bin', 'private_key.pem')
TOKEN_FIELDS = ('apple_app_token', 'apple_user_token', 'yandex_music_token', 'yandex_x_token', 'station_device_id')


def validate_host(value):
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.\-]*', value):
        raise ValueError('Укажите IPv4 или имя хоста без пробелов и SSH-параметров.')
    return value


def extract_package(data, destination):
    """Reject links and traversal before writing anything from a release archive."""
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        members=archive.getmembers()
        if sum(m.size for m in members)>100*1024*1024:
            raise ValueError('Распакованный архив превышает 100 МиБ.')
        for member in members:
            parts=Path(member.name).parts
            if (not parts or parts[0]!='alice-music' or Path(member.name).is_absolute()
                or '..' in parts or not (member.isfile() or member.isdir())):
                raise ValueError('Небезопасный путь или ссылка в архиве.')
            if len(parts)>1 and parts[1]=='private':
                raise ValueError('Релиз не должен содержать личный профиль.')
        for member in members:
            target=destination.joinpath(*Path(member.name).parts)
            if member.isdir():target.mkdir(parents=True,exist_ok=True)
            else:
                target.parent.mkdir(parents=True,exist_ok=True)
                with archive.extractfile(member) as source,target.open('wb') as output:
                    shutil.copyfileobj(source,output)
                target.chmod(member.mode & 0o755)
    root=destination/'alice-music'
    for name in ('service.py','launch.sh','station_runner.py','S99zzalice-music','installer_remote.py'):
        if not (root/name).is_file():raise ValueError('Неполный релиз: '+name)
    return root


def prepare_profile(profile, root):
    profile=Path(profile).expanduser().resolve()
    for name in REQUIRED:
        if not (profile/name).is_file():raise ValueError('В личном профиле отсутствует '+name)
    config=json.loads((profile/'config.json').read_text())
    for name in TOKEN_FIELDS:
        value=config.get(name,'')
        if not isinstance(value,str) or not value or value.startswith('YOUR_'):
            raise ValueError('Заполните личное поле '+name)
    defaults=json.loads((root/'config.example.json').read_text())
    defaults.update(config)
    defaults.update(station_host='127.0.0.1',station_port=1961,bind_host='127.0.0.1',port=8099,
                    ffmpeg='/opt/bin/ffmpeg',recognition_port=9875,open_skill_on_start=False)
    if not defaults.get('agent_token') or defaults['agent_token'].startswith('YOUR_'):
        defaults['agent_token']=secrets.token_urlsafe(32)
    private=root/'private';private.mkdir(mode=0o700,exist_ok=True)
    (private/'config.json').write_text(json.dumps(defaults,ensure_ascii=False,indent=2)+'\n')
    for name in REQUIRED[1:]:shutil.copyfile(profile/name,private/name)
    if (profile/'queue.json').is_file():shutil.copyfile(profile/'queue.json',private/'queue.json')
    for file in private.iterdir():file.chmod(0o600)


class Remote:
    def __init__(self,host,port):
        self.args=['ssh','-p',str(port),'-o','ConnectTimeout=10','root@'+validate_host(host)]
    def run(self,command,data=None,capture=False):
        result=subprocess.run(self.args+[command],input=data,stdout=subprocess.PIPE if capture else None,
                              stderr=subprocess.PIPE if capture else None)
        if result.returncode:
            # Do not include remote configuration or credential-bearing stdout in errors.
            raise RuntimeError('SSH-команда не выполнена. Проверьте доступ к колонке и журнал установки.')
        return result.stdout if capture else None
    def python(self,code,capture=False):
        return self.run('/opt/bin/python3 -c '+shlex.quote(code),capture=capture)


def main():
    parser=argparse.ArgumentParser(description='Alice Apple Music — установка на Станцию 2 по SSH')
    parser.add_argument('--host',help='IP-адрес Станции 2')
    parser.add_argument('--port',type=int,default=2222)
    parser.add_argument('--archive',type=Path,help='Локальный ARM64 архив вместо скачивания релиза')
    parser.add_argument('--profile',type=Path,help='Личный профиль для первой установки; при обновлении можно не указывать')
    parser.add_argument('--check',action='store_true',help='Только проверить совместимость, ничего не менять')
    parser.add_argument('--skip-deps',action='store_true',help='Не запускать opkg; зависимости уже должны быть установлены')
    args=parser.parse_args()
    if not shutil.which('ssh'):parser.error('Нужен OpenSSH client.')
    if not 1<=args.port<=65535:parser.error('Некорректный SSH-порт.')
    host=args.host or input('IP Станции 2: ').strip()
    remote=Remote(host,args.port)
    print('\nAlice Apple Music · Станция 2\nПроверяю root, модель, Python и свободное место…',flush=True)
    info=json.loads(remote.python("""import os,json,platform,sys,shutil
from pathlib import Path
print(json.dumps({'root':os.geteuid()==0,'arch':platform.machine(),'python':list(sys.version_info[:2]),
 'compatible':Path('/proc/device-tree/compatible').read_bytes().decode().split('\\x00'),
 'free':shutil.disk_usage('/data').free,'configured':all((Path('/data/opt/alice-music/private')/n).is_file() for n in ('config.json','client_id.bin','private_key.pem'))}))""",capture=True))
    if not info['root'] or info['arch']!='aarch64' or 'yandex,station-midi' not in info['compatible']:
        raise ValueError('Этот релиз предназначен для Станции 2 с root и ARM64.')
    if info['python']!=[3,13]:raise ValueError('Нужен Entware Python 3.13 в /opt/bin/python3.')
    print(f"Совместимо · свободно {info['free']//1048576} МиБ · профиль: {'есть' if info['configured'] else 'не настроен'}",flush=True)
    if args.check:return
    if not args.profile and not info['configured']:
        raise ValueError('Для первой установки нужен --profile /путь/к/личной/папке. См. docs/AUTH.md. Токены в чат не отправляйте.')
    if info['free']<55*1024*1024:raise ValueError('Для установки и отката нужно не менее 55 МиБ свободного места.')
    print('Подготавливаю ARM64-пакет…',flush=True)
    if args.archive:data=args.archive.read_bytes()
    else:
        with urlopen(RELEASE+'manifest.json',timeout=30) as response:manifest=json.load(response)
        if manifest.get('archive')!='alice-music-station-arm64.tar.gz':raise ValueError('Неподдерживаемый манифест релиза.')
        with urlopen(RELEASE+manifest['archive'],timeout=90) as response:data=response.read(30*1024*1024+1)
        if hashlib.sha256(data).hexdigest()!=manifest['sha256']:raise ValueError('SHA-256 архива не совпадает.')
    with tempfile.TemporaryDirectory(prefix='alice-music-install-') as temporary:
        root=extract_package(data,Path(temporary))
        if args.profile:prepare_profile(args.profile,root)
        if not args.skip_deps:
            print('Устанавливаю FFmpeg, сертификаты и модули Python через Entware…',flush=True)
            remote.run('/opt/bin/opkg install ffmpeg ca-bundle python3-asyncio python3-codecs python3-email python3-openssl python3-sqlite3 python3-urllib python3-uuid python3-ctypes')
        payload=io.BytesIO()
        with tarfile.open(fileobj=payload,mode='w') as archive:
            for path in root.rglob('*'):
                if path.is_file():archive.add(path,arcname=str(path.relative_to(root)))
        stage='/data/opt/.alice-music-stage-'+secrets.token_hex(6)
        remote.run('/opt/bin/python3 -c '+shlex.quote(
            "import tarfile,sys;from pathlib import Path;p=Path("+repr(stage)+");p.mkdir(mode=0o700);tarfile.open(fileobj=sys.stdin.buffer,mode='r|').extractall(p,filter='data')"),payload.getvalue())
        print('Сохраняю профиль и предыдущую версию, переключаю плеер…',flush=True)
        remote.run('/opt/bin/python3 '+shlex.quote(stage+'/installer_remote.py')+' '+shlex.quote(stage))
    print('\nГотово. Скажите: «Алиса, включи песню…».\nПрофиль не публиковался. Предыдущая версия: /data/opt/alice-music.previous\n',flush=True)


if __name__=='__main__':
    try:main()
    except (ValueError,RuntimeError,OSError,KeyboardInterrupt) as error:
        print('\nУстановка остановлена: '+str(error),file=sys.stderr);sys.exit(1)
