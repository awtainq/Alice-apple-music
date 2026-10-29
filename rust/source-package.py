#!/usr/bin/env python3
"""Bundle native corresponding source, including all Cargo.lock dependencies."""
import argparse,gzip,io,pathlib,subprocess,tarfile,tempfile,tomllib
ROOT=pathlib.Path(__file__).resolve().parent

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=pathlib.Path);a=p.parse_args()
    version=tomllib.loads((ROOT/'Cargo.toml').read_text())['package']['version'];name='alice-apple-music-'+version+'-rust-source'
    output=a.output or ROOT.parent/'dist'/(name+'.tar.gz');output.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='alice-music-source-') as tmp:
        stage=pathlib.Path(tmp);vendor=stage/'vendor'
        subprocess.run(['cargo','vendor','--locked','--manifest-path',str(ROOT/'Cargo.toml'),str(vendor)],check=True,stdout=subprocess.DEVNULL)
        files=[]
        for base in[ROOT,ROOT.parent/'third_party',ROOT.parent/'docs']:
            for f in sorted(base.rglob('*')):
                if f.is_file() and not any(x in f.parts for x in['target','__pycache__','.git']):files.append((f.relative_to(ROOT.parent).as_posix(),f.read_bytes()))
        for f in sorted(vendor.rglob('*')):
            if f.is_file():files.append((f.relative_to(stage).as_posix(),f.read_bytes()))
        files.append(('.cargo/config.toml',b'[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "vendor"\n'))
        files.append(('BUILD.txt',b'Native GPL-3.0-only corresponding source. Dependencies are vendored.\nFrom this directory: cargo build --offline --locked --release --manifest-path rust/Cargo.toml\nFor Station add --target aarch64-unknown-linux-musl (installed musl cross-toolchain or cargo-zigbuild required).\nBuild toolchains themselves are not bundled. See rust/README.md and rust/NOTICE.\n'))
        for f in['LICENSE','NOTICE','VERSION','README.md']:
            files.append((f,(ROOT.parent/f).read_bytes()))
        with output.open('wb') as dst,gzip.GzipFile(fileobj=dst,mode='wb',filename='',mtime=0) as gz,tarfile.open(fileobj=gz,mode='w') as archive:
            for rel,data in files:
                info=tarfile.TarInfo(name+'/'+rel);info.mode=0o644;info.size=len(data);archive.addfile(info,io.BytesIO(data))
    print(output)
if __name__=='__main__':main()
