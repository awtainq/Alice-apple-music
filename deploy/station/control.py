import json,sys
from pathlib import Path
from urllib.request import Request,urlopen
base=Path(__file__).resolve().parent;c=json.loads((base/'private/config.json').read_text())
body=None;headers={'Authorization':'Bearer '+c['agent_token']}
if len(sys.argv)>1:
 body=json.dumps({'action':sys.argv[1],'query':' '.join(sys.argv[2:])}).encode();headers['Content-Type']='application/json'
with urlopen(Request(f'http://{c["bind_host"]}:{c["port"]}/api',data=body,headers=headers),timeout=100) as r:
 data=json.load(r)
 print(json.dumps(data,ensure_ascii=False))
