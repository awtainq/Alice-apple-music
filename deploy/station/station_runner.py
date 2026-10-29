"""Single-process Station entry point with bounded RAM logs."""
import logging
from logging.handlers import RotatingFileHandler
from pathlib import Path
import runpy
import sys

base=Path(__file__).resolve().parent
sys.path.insert(0,str(base/'vendor'))
log=logging.getLogger('alice-music');log.setLevel(logging.INFO)
handler=RotatingFileHandler('/tmp/alice-music.log',maxBytes=262144,backupCount=1,encoding='utf-8')
handler.setFormatter(logging.Formatter('%(asctime)s %(message)s'));log.addHandler(handler)
class Output:
 def __init__(self):self.pending=''
 def write(self,text):
  self.pending+=text
  while '\n' in self.pending:
   line,self.pending=self.pending.split('\n',1)
   if line:log.info(line)
  return len(text)
 def flush(self):
  if self.pending:log.info(self.pending);self.pending=''
sys.stdout=Output();sys.stderr=Output()
sys.argv=[str(base/'service.py'),'--private',str(base/'private'),'--cache','/tmp/alice-music-cache','--no-cloud','--local-voice']
runpy.run_module('service',run_name='__main__')
