"""Authenticated control of your own player. No tokens in command arguments."""
import argparse,json
from pathlib import Path
from urllib.request import Request,urlopen
p=argparse.ArgumentParser();p.add_argument('--private',default='private')
p.add_argument('action',nargs='?',choices=['play','next','previous','pause','resume','similar','now'])
p.add_argument('query',nargs='*');a=p.parse_args()
c=json.loads((Path(a.private)/'config.json').read_text())
headers={'Authorization':'Bearer '+c['agent_token']};body=None
if a.action:
 body=json.dumps({'action':a.action,'query':' '.join(a.query)}).encode();headers['Content-Type']='application/json'
url=f'http://{c["bind_host"]}:{c["port"]}/api'
with urlopen(Request(url,data=body,headers=headers),timeout=100) as r:
 print(json.dumps(json.load(r),ensure_ascii=False,indent=2))
