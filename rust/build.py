#!/usr/bin/env python3
"""Build ARM64 musl without embedding the builder's personal directory paths."""
import argparse,os,pathlib,subprocess
ROOT=pathlib.Path(__file__).resolve().parent
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--zig',action='store_true');a=p.parse_args()
env=os.environ.copy();home=str(pathlib.Path.home());repo=str(ROOT.parent)
env['RUSTFLAGS']=env.get('RUSTFLAGS','')+f' --remap-path-prefix={home}=/build/home --remap-path-prefix={repo}=/build/app'
env['CFLAGS']=env.get('CFLAGS','')+f' -ffile-prefix-map={home}=/build/home -ffile-prefix-map={repo}=/build/app'
command=['cargo','zigbuild' if a.zig else 'build','--locked','--release','--target','aarch64-unknown-linux-musl','--manifest-path',str(ROOT/'Cargo.toml')]
subprocess.run(command,env=env,check=True)
