"""Generate private relay secrets without printing them."""
import argparse,json,secrets
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--private',default='private');args=p.parse_args()
d=Path(args.private);d.mkdir(parents=True,exist_ok=True);d.chmod(0o700)
c=json.loads((d/'config.json').read_text())
for field,filename in [('webhook_token','webhook-token.txt'),('agent_token','agent-token.txt'),('bind_code','bind-code.txt')]:
 value=str(c.get(field,''))
 if not value or value.startswith(('YOUR_','REPLACE_')):
  value=str(10**23+secrets.randbelow(9*10**23)) if field=='bind_code' else secrets.token_urlsafe(32)
 c[field]=value;f=d/filename;f.write_text(value);f.chmod(0o600)
f=d/'config.json';f.write_text(json.dumps(c,ensure_ascii=False,indent=2)+'\n');f.chmod(0o600)
print('Private relay secrets saved. Values were not printed.')
