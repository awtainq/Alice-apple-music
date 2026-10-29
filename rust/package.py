#!/usr/bin/env python3
"""Build a reproducible Luna .yap from an explicitly selected ARM64 binary."""
import argparse,gzip,hashlib,io,pathlib,tarfile,tomllib
ROOT=pathlib.Path(__file__).resolve().parent

def build(binary,output=None):
    binary=pathlib.Path(binary);raw=binary.read_bytes()
    if raw[:4]!=b'\x7fELF' or raw[4]!=2 or int.from_bytes(raw[18:20],'little')!=183:
        raise ValueError('Expected an ARM64 ELF binary, built for aarch64-unknown-linux-musl')
    manifest=(ROOT/'luna/plugin.toml').read_bytes()
    version=tomllib.loads(manifest.decode())['version']
    output=pathlib.Path(output) if output else ROOT.parent/'dist'/f'alice-apple-music-luna-{version}-aarch64.yap'
    output.parent.mkdir(parents=True,exist_ok=True)
    files={'plugin.toml':(manifest,0o644),'bin/alice-apple-music':(raw,0o755),
           'README.md':((ROOT/'README.md').read_bytes(),0o644),
           'LICENSE':((ROOT/'LICENSE').read_bytes(),0o644),
           'NOTICE':((ROOT/'NOTICE').read_bytes(),0o644)}
    with output.open('wb') as dst,gzip.GzipFile(fileobj=dst,mode='wb',filename='',mtime=0) as gz,tarfile.open(fileobj=gz,mode='w') as archive:
        for name,(data,mode) in files.items():
            info=tarfile.TarInfo(name);info.mode=mode;info.mtime=0;info.size=len(data);archive.addfile(info,io.BytesIO(data))
    digest=hashlib.sha256(output.read_bytes()).hexdigest();output.with_suffix(output.suffix+'.sha256').write_text(digest+'  '+output.name+'\n')
    print(output);print('SHA256 '+digest)
    return output
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=pathlib.Path,default=ROOT/'target/aarch64-unknown-linux-musl/release/alice-apple-music');p.add_argument('--output',type=pathlib.Path);a=p.parse_args();build(a.binary,a.output)
