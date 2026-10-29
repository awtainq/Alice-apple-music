import importlib.util
import io
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
import install
spec=importlib.util.spec_from_file_location('remote_install',ROOT/'scripts/installer_remote.py')
remote=importlib.util.module_from_spec(spec);spec.loader.exec_module(remote)

class Installer(unittest.TestCase):
 def archive(self,name,kind=tarfile.REGTYPE):
  stream=io.BytesIO()
  with tarfile.open(fileobj=stream,mode='w:gz') as a:
   member=tarfile.TarInfo(name);member.type=kind;member.linkname='/etc/passwd';member.size=0;a.addfile(member)
  return stream.getvalue()
 def test_reject_traversal_links_and_private_payload(self):
  with tempfile.TemporaryDirectory() as d:
   for name,kind in [('alice-music/../../outside',tarfile.REGTYPE),('/etc/passwd',tarfile.REGTYPE),('alice-music/key',tarfile.SYMTYPE),('alice-music/private/config.json',tarfile.REGTYPE)]:
    with self.assertRaises(ValueError):install.extract_package(self.archive(name,kind),Path(d))
 def test_host_cannot_supply_ssh_options_or_shell(self):
  for host in ['-oProxyCommand=x','host;whoami','root@host','host name','$(id)']:
   with self.assertRaises(ValueError):install.validate_host(host)
  self.assertEqual(install.validate_host('192.168.1.100'),'192.168.1.100')
 def test_failed_health_check_restores_previous_app_and_backup(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);target=root/'app';stage=root/'stage';backup=root/'previous'
   for p,v in [(target,'working'),(stage,'broken'),(backup,'older')]:p.mkdir();(p/'version').write_text(v)
   starts=[]
   def start(p):starts.append((p/'version').read_text())
   def fail(p):raise RuntimeError('health failed')
   with self.assertRaises(RuntimeError):remote.switch_install(stage,target,backup,start,lambda p:None,fail)
   self.assertEqual((target/'version').read_text(),'working')
   self.assertEqual((backup/'version').read_text(),'older')
   self.assertEqual(starts,['broken','working'])
 def test_success_keeps_one_previous_version(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);target=root/'app';stage=root/'stage';backup=root/'previous'
   for p,v in [(target,'working'),(stage,'new'),(backup,'older')]:p.mkdir();(p/'version').write_text(v)
   remote.switch_install(stage,target,backup,lambda p:None,lambda p:None,lambda p:None)
   self.assertEqual((target/'version').read_text(),'new');self.assertEqual((backup/'version').read_text(),'working')
   self.assertFalse((root/'previous.obsolete').exists())
