#!/usr/bin/env python3
"""Build a reproducible, credential-free ARM64 release. Requires pip only for --download."""
import argparse
from email.parser import BytesParser
import gzip
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
ROOT=Path(__file__).resolve().parents[1]


def unpack_wheel(path,vendor):
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            name=Path(info.filename)
            if name.is_absolute() or '..' in name.parts:raise ValueError('Unsafe wheel path')
            if info.is_dir() or '__pycache__' in name.parts or name.suffix=='.pyc':continue
            if name.parts[0].endswith('.data'):continue
            if name.parts[:2]==('Crypto','SelfTest'):continue
            target=vendor/name;target.parent.mkdir(parents=True,exist_ok=True)
            target.write_bytes(archive.read(info))


def build(vendor,output):
    version=(ROOT/'VERSION').read_text().strip();output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='alice-release-') as temporary:
        app=Path(temporary)/'alice-music';app.mkdir()
        for name in ['airplay.py','apple.py','engine.py','service.py','station_client.py','voice.py','LICENSE','NOTICE','README.md','VERSION']:
            shutil.copy2(ROOT/name,app/name)
        for name in ['launch.sh','S99zzalice-music','station_runner.py','control.py','config.example.json']:
            shutil.copy2(ROOT/'deploy/station'/name,app/name)
        shutil.copy2(ROOT/'scripts/installer_remote.py',app/'installer_remote.py')
        shutil.copytree(ROOT/'third_party',app/'third_party')
        shutil.copytree(ROOT/'docs',app/'docs')
        shutil.copytree(vendor,app/'vendor',ignore=shutil.ignore_patterns('__pycache__','*.pyc','SelfTest'))
        (app/'check_runtime.py').write_text("import sys\nfrom pathlib import Path\nsys.path.insert(0,str(Path(__file__).resolve().parent/'vendor'))\nimport aiohttp,apple,engine,service,station_client,voice,airplay,dbus_next\nfrom Crypto.Cipher import AES\nAES.new(bytes(16),AES.MODE_ECB).encrypt(bytes(16))\nprint('ARM runtime: OK')\n")
        paths=sorted(p for p in app.rglob('*') if p.is_file())
        if any('private' in p.relative_to(app).parts or p.name in ('config.json','queue.json','private_key.pem','client_id.bin') for p in paths):raise ValueError('Private material in release')
        archive=output/'alice-music-station-arm64.tar.gz'
        with archive.open('wb') as destination,gzip.GzipFile(fileobj=destination,mode='wb',mtime=0,filename='') as gz,tarfile.open(fileobj=gz,mode='w') as tar:
            for path in paths:
                data=path.read_bytes();info=tarfile.TarInfo(str(Path('alice-music')/path.relative_to(app)))
                info.size=len(data);info.mode=0o755 if path.name in ('launch.sh','S99zzalice-music') else 0o644
                tar.addfile(info,io.BytesIO(data))
        digest=hashlib.sha256(archive.read_bytes()).hexdigest()
        manifest={'version':version,'archive':archive.name,'sha256':digest,'platform':'aarch64-linux-gnu','python':'3.13','expanded_bytes':sum(p.stat().st_size for p in paths)}
        (output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
        (output/'SHA256SUMS').write_text(digest+'  '+archive.name+'\n')
        shutil.copy2(ROOT/'install.py',output/'install.py')
        print(json.dumps(manifest,indent=2))


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--vendor',type=Path)
    parser.add_argument('--download',action='store_true');parser.add_argument('--output',type=Path,default=ROOT/'dist')
    args=parser.parse_args()
    if args.vendor:build(args.vendor,args.output);return
    if not args.download:parser.error('Use --vendor ARM_VENDOR_DIRECTORY or --download')
    with tempfile.TemporaryDirectory(prefix='alice-wheels-') as temporary:
        base=Path(temporary);pure=base/'pure';wheels=base/'wheels';vendor=base/'vendor';vendor.mkdir()
        subprocess.run([sys.executable,'-m','pip','wheel','--no-deps','--wheel-dir',str(pure),'construct==2.8.8'],check=True)
        subprocess.run([sys.executable,'-m','pip','download','--dest',str(wheels),'--find-links',str(pure),
                        '--platform','manylinux2014_aarch64','--python-version','3.13','--implementation','cp','--abi','cp313',
                        '--only-binary=:all:','-r',str(ROOT/'requirements-station.lock')],check=True)
        for wheel in sorted(wheels.glob('*.whl')):unpack_wheel(wheel,vendor)
        build(vendor,args.output)


if __name__=='__main__':main()
