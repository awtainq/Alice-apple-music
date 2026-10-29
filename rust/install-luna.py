#!/usr/bin/env python3
"""Install/upgrade the native Luna package through SSH; Python runs only on this PC."""
import argparse,contextlib,gzip,hashlib,io,json,pathlib,shlex,socket,subprocess,tarfile,time,tomllib,urllib.error,urllib.request,uuid
ID='alice-apple-music'
DATA='/data/opt/luna/data'
PRIVATE=DATA+'/apple-music-private'
LEGACY='/data/opt/alice-music'
MARKER=LEGACY+'/.managed-by-luna'
PROFILE_FILES=('config.json','client_id.bin','private_key.pem','queue.json')

def q(value):return shlex.quote(str(value))
class Connection:
    def __init__(self,args):
        self.args=args
        self.base=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10','-p',str(args.port),args.host if '@' in args.host else 'root@'+args.host]
    def run(self,command,data=None):
        p=subprocess.run(self.base+[command],input=data,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        if p.returncode:raise RuntimeError('SSH operation failed (remote output withheld to protect profile data).')
        return p.stdout
    @contextlib.contextmanager
    def tunnel(self):
        with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
        with socket.socket() as s:s.bind(('127.0.0.1',0));player_port=s.getsockname()[1]
        args=self.base[:-1]+['-o','ExitOnForwardFailure=yes','-N','-L',f'127.0.0.1:{port}:127.0.0.1:{self.args.luna_port}','-L',f'127.0.0.1:{player_port}:127.0.0.1:8099',self.base[-1]]
        p=subprocess.Popen(args,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        try:
            for _ in range(100):
                if p.poll() is not None:raise RuntimeError('SSH tunnel failed.')
                try:
                    with socket.create_connection(('127.0.0.1',port),.2):break
                except OSError:time.sleep(.1)
            else:raise RuntimeError('SSH tunnel did not open.')
            yield f'http://127.0.0.1:{port}',f'http://127.0.0.1:{player_port}'
        finally:
            p.terminate()
            try:p.wait(timeout=5)
            except subprocess.TimeoutExpired:p.kill();p.wait()

def api(base,path,data=None,method=None,token=None):
    headers={}
    if token:headers['Authorization']='Bearer '+token
    if isinstance(data,dict):data=json.dumps(data).encode();headers['Content-Type']='application/json'
    with urllib.request.urlopen(urllib.request.Request(base+path,data=data,headers=headers,method=method),timeout=110) as r:
        raw=r.read();return json.loads(raw) if raw else None

def upload(base,raw):
    boundary='alice'+uuid.uuid4().hex
    body=(f'--{boundary}\r\nContent-Disposition: form-data; name="package"; filename="apple-music.yap"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode()+raw+f'\r\n--{boundary}--\r\n'.encode())
    with urllib.request.urlopen(urllib.request.Request(base+'/api/plugins',body,{'Content-Type':'multipart/form-data; boundary='+boundary}),timeout=110) as r:r.read()

def validate_package(path):
    raw=path.read_bytes()
    if len(raw)>24*1024*1024:raise ValueError('Package exceeds 24 MiB.')
    checksum=path.with_suffix(path.suffix+'.sha256')
    if checksum.exists() and checksum.read_text().split()[0]!=hashlib.sha256(raw).hexdigest():raise ValueError('Package SHA256 mismatch.')
    with tarfile.open(fileobj=io.BytesIO(raw),mode='r:gz') as archive:
        files=archive.getmembers()
        allowed={'plugin.toml','bin/alice-apple-music','README.md','LICENSE','NOTICE'}
        if {f.name for f in files}!=allowed or any(not f.isfile() for f in files):raise ValueError('Unexpected package contents.')
        if sum(f.size for f in files)>32*1024*1024:raise ValueError('Expanded package exceeds 32 MiB.')
        manifest=tomllib.loads(archive.extractfile('plugin.toml').read().decode())
        if manifest.get('id')!=ID or manifest.get('launch',{}).get('kind')!='binary':raise ValueError('Unexpected Luna package id/type.')
        binary=archive.extractfile('bin/alice-apple-music').read(20)
        if binary[:4]!=b'\x7fELF' or binary[4]!=2 or int.from_bytes(binary[18:20],'little')!=183:raise ValueError('Expected ARM64 ELF.')
    return raw

def stop(base):
    for suffix in ['/service/disable','/stop']:
        try:api(base,'/api/plugins/'+ID+suffix,{},'POST')
        except urllib.error.HTTPError as e:
            if e.code!=404:raise
    time.sleep(2)
def start(base):
    api(base,'/api/plugins/'+ID+'/service/enable',{},'POST')

def profile_archive(profile):
    out=io.BytesIO()
    with tarfile.open(fileobj=out,mode='w') as archive:
        for name in PROFILE_FILES:
            p=profile/name
            if not p.is_file():
                if name=='queue.json':continue
                raise ValueError('Missing profile file: '+name)
            info=tarfile.TarInfo(name);data=p.read_bytes();info.size=len(data);info.mode=0o600;archive.addfile(info,io.BytesIO(data))
    return out.getvalue()

def install(args):
    raw=validate_package(args.package);c=Connection(args);print('Package verified; connecting to Luna.',flush=True)
    c.run('test "$(uname -m)" = aarch64 && test -x /opt/bin/ffmpeg && test -d '+q(DATA))
    with c.tunnel() as (base,player):
        packages=api(base,'/api/plugins')
        if isinstance(packages,dict):packages=packages.get('plugins',[])
        old=next((p for p in packages if p.get('manifest',{}).get('id')==ID),None)
        backup=None
        if old:
            backup=gzip.compress(c.run('tar -cf - -C '+q(DATA+'/plugins/'+ID)+' .'),mtime=0)
        python_running=c.run(f'if [ -x {LEGACY}/launch.sh ]; then {LEGACY}/launch.sh status; fi').strip()==b'running'
        marker_existed=c.run(f'if [ -e {MARKER} ]; then echo yes; fi').strip()==b'yes'
        # Keep a byte-exact launcher backup for rollback; only start is guarded.
        c.run(f'if [ -x {LEGACY}/launch.sh ] && [ ! -e {LEGACY}/launch.before-rust.sh ]; then cp -p {LEGACY}/launch.sh {LEGACY}/launch.before-rust.sh; fi')
        if args.profile:profile=profile_archive(args.profile)
        else:profile=None
        profile_existed=c.run(f'if [ -f {PRIVATE}/config.json ]; then echo yes; fi').strip()==b'yes'
        changed=False;stopped_old=False
        try:
            if old:stop(base);stopped_old=True
            if python_running:c.run(LEGACY+'/launch.sh stop');time.sleep(2)
            c.run(f'umask 077; mkdir -p {PRIVATE}; chmod 700 {PRIVATE}')
            if profile:
                if profile_existed:raise ValueError('Profile exists; refusing to overwrite it. Omit --profile for upgrades.')
                c.run('tar -xf - -C '+q(PRIVATE),profile)
            elif not profile_existed:
                c.run(f'test -f {LEGACY}/private/config.json; for f in config.json client_id.bin private_key.pem queue.json; do if [ -f {LEGACY}/private/$f ]; then cp -p {LEGACY}/private/$f {PRIVATE}/$f; fi; done')
            # Copy the final queue after the old service checkpointed on shutdown.
            elif python_running:c.run(f'if [ -f {LEGACY}/private/queue.json ]; then cp -p {LEGACY}/private/queue.json {PRIVATE}/queue.json; fi')
            c.run(f'chmod 600 {PRIVATE}/*; test -s {PRIVATE}/config.json; test -s {PRIVATE}/client_id.bin; test -s {PRIVATE}/private_key.pem')
            config=json.loads(c.run('cat '+q(PRIVATE+'/config.json')))
            if config.get('port',8099)!=8099:raise ValueError('This Luna installer expects player port 8099.')
            if c.run(f'if [ -f {LEGACY}/launch.sh ]; then echo yes; fi').strip()==b'yes':
                launcher=c.run('cat '+LEGACY+'/launch.sh')
                guard=f'  [ ! -e {q(MARKER)} ] || exit 0\n'.encode()
                if guard not in launcher:
                    if b' start)\n' not in launcher:raise ValueError('Legacy launcher format is unknown; not changing it.')
                    launcher=launcher.replace(b' start)\n',b' start)\n'+guard,1)
                    c.run('cat > '+LEGACY+'/launch.sh && chmod 700 '+LEGACY+'/launch.sh',launcher)
                c.run('touch '+MARKER)
            changed=True;print('Profile preserved; uploading and starting Rust service.',flush=True);upload(base,raw);start(base)
            for _ in range(45):
                try:
                    status=api(player,'/api',token=config['agent_token'])
                    if status.get('implementation')=='rust' and status.get('station_connected'):break
                except (OSError,ValueError):pass
                time.sleep(1)
            else:raise RuntimeError('Rust player did not connect to the Station within 45 seconds.')
            print('Installed Alice × Apple Music '+status['version']+' in Luna; Station connected.')
            print('Profile and queue preserved; previous files, if present, retained for rollback.')
        except Exception:
            if changed or stopped_old:
                try:
                    stop(base)
                    if backup and changed:upload(base,backup)
                    if old and old.get('service_enabled'):start(base)
                    elif old and old.get('running'):api(base,'/api/plugins/'+ID+'/service/start',{},'POST')
                except Exception:print('Could not restore previous Luna package; inspect Luna service state.')
            if not marker_existed:c.run('rm -f '+MARKER)
            if python_running:c.run(LEGACY+'/launch.sh start')
            raise

def rollback(args):
    c=Connection(args)
    c.run(f'test -x {LEGACY}/launch.sh && test -x /opt/bin/python3')
    with c.tunnel() as (base,_):stop(base)
    c.run(f'test -x {LEGACY}/launch.sh; if [ -f {PRIVATE}/queue.json ]; then cp -p {PRIVATE}/queue.json {LEGACY}/private/queue.json; fi; rm -f {MARKER}; {LEGACY}/launch.sh start')
    print('Luna music disabled; previous Python player started with current queue.')

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--host',required=True);p.add_argument('--port',type=int,default=2222);p.add_argument('--luna-port',type=int,default=8098);p.add_argument('--package',type=pathlib.Path);p.add_argument('--profile',type=pathlib.Path);p.add_argument('--rollback-python',action='store_true');a=p.parse_args()
    if not a.rollback_python and not a.package:p.error('--package is required for installation')
    try:rollback(a) if a.rollback_python else install(a)
    except Exception as e:raise SystemExit('Installation failed: '+type(e).__name__+'. Profile data and server responses are withheld. Check Luna logs locally.')
if __name__=='__main__':main()
