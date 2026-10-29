import contextlib,importlib.util,io,pathlib,tarfile,tempfile,types,unittest
from unittest.mock import patch
ROOT=pathlib.Path(__file__).resolve().parents[1]
def module(name,path):
    spec=importlib.util.spec_from_file_location(name,path);obj=importlib.util.module_from_spec(spec);spec.loader.exec_module(obj);return obj
installer=module('luna_installer',ROOT/'install-luna.py')
package=module('luna_packager',ROOT/'package.py')
class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.path=pathlib.Path(self.temp.name)
        self.binary=self.path/'player';self.binary.write_bytes(b'\x7fELF\x02'+bytes(13)+(183).to_bytes(2,'little'))
    def test_reproducible_and_no_profile(self):
        one=package.build(self.binary,self.path/'one.yap');two=package.build(self.binary,self.path/'two.yap')
        self.assertEqual(one.read_bytes(),two.read_bytes());installer.validate_package(one)
        with tarfile.open(one) as arc:self.assertFalse(any('private' in f.name or 'target' in f.name for f in arc))
    def test_checksum_mismatch_rejected(self):
        one=package.build(self.binary,self.path/'one.yap');one.with_suffix('.yap.sha256').write_text('00 invalid\n')
        with self.assertRaisesRegex(ValueError,'SHA256'):installer.validate_package(one)
    def test_personal_build_path_rejected(self):
        self.binary.write_bytes(self.binary.read_bytes()+str(pathlib.Path.home()).encode())
        with self.assertRaisesRegex(ValueError,'Personal build paths'):package.build(self.binary,self.path/'leak.yap')
    def test_host_binary_rejected(self):
        self.binary.write_bytes(b'not an ARM ELF file................')
        with self.assertRaisesRegex(ValueError,'ARM64'):package.build(self.binary,self.path/'bad.yap')
    def test_unexpected_archive_entries_rejected(self):
        raw=io.BytesIO()
        with tarfile.open(fileobj=raw,mode='w:gz') as arc:
            info=tarfile.TarInfo('../escape');info.size=1;arc.addfile(info,io.BytesIO(b'x'))
        path=self.path/'attack.yap';path.write_bytes(raw.getvalue())
        with self.assertRaisesRegex(ValueError,'contents'):installer.validate_package(path)
class TransactionTests(unittest.TestCase):
    def test_failed_profile_upgrade_restarts_previous_services(self):
        calls=[];endpoints=[]
        class FakeConnection:
            def __init__(self,args):pass
            def run(self,cmd,data=None):
                calls.append(cmd)
                if 'launch.sh status' in cmd:return b'running'
                if 'if [ -f '+installer.PRIVATE in cmd:return b'yes'
                return b''
            @contextlib.contextmanager
            def tunnel(self):yield 'luna','player'
        def api(base,path,*args,**kwargs):
            endpoints.append(path)
            if path=='/api/plugins':return [{'manifest':{'id':installer.ID},'running':True,'service_enabled':True}]
        args=types.SimpleNamespace(package=None,profile=pathlib.Path('fixture'))
        with patch.object(installer,'validate_package',return_value=b'package'),patch.object(installer,'Connection',FakeConnection),patch.object(installer,'api',api),patch.object(installer,'profile_archive',return_value=b'profile'),patch.object(installer.time,'sleep'):
            with self.assertRaisesRegex(ValueError,'Profile exists'):installer.install(args)
        self.assertIn('/api/plugins/'+installer.ID+'/service/enable',endpoints)
        self.assertIn(installer.LEGACY+'/launch.sh start',calls)
    def test_rollback_checks_python_before_stopping_rust(self):
        class MissingPython:
            def __init__(self,args):pass
            def run(self,cmd):raise RuntimeError('Missing Python')
        with patch.object(installer,'Connection',MissingPython),patch.object(installer,'stop') as stop:
            with self.assertRaises(RuntimeError):installer.rollback(None)
            stop.assert_not_called()
if __name__=='__main__':unittest.main()
