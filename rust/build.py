#!/usr/bin/env python3
"""Build ARM64 musl without embedding the builder's personal directory paths."""
import argparse,os,pathlib,subprocess
ROOT=pathlib.Path(__file__).resolve().parent
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--zig',action='store_true');a=p.parse_args()
env=os.environ.copy()
prefixes=[(pathlib.Path.home(),'/build/home'),(pathlib.Path.cwd(),'/build/workspace'),
          (pathlib.Path(env.get('CARGO_HOME',pathlib.Path.home()/'.cargo')),'/build/cargo'),
          (pathlib.Path(env.get('RUSTUP_HOME',pathlib.Path.home()/'.rustup')),'/build/rustup'),
          (ROOT.parent,'/build/app')]
env['RUSTFLAGS']=env.get('RUSTFLAGS','')+''.join(f' --remap-path-prefix={source}={dest}' for source,dest in prefixes)
env['CFLAGS']=env.get('CFLAGS','')+''.join(f' -ffile-prefix-map={source}={dest}' for source,dest in prefixes)
command=['cargo','zigbuild' if a.zig else 'build','--locked','--release','--target','aarch64-unknown-linux-musl','--manifest-path',str(ROOT/'Cargo.toml')]
subprocess.run(command,env=env,check=True)
