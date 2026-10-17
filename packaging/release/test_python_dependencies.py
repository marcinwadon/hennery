"""Every runner exercises the stdlib helpers without optional site packages."""
from pathlib import Path
import subprocess
import sys
import textwrap
import unittest


class PythonDependencies(unittest.TestCase):
    def bare(self, program):
        root=Path(__file__).resolve().parent
        prelude=f'''import sys, importlib.util
sys.path.insert(0,{str(root)!r})
assert importlib.util.find_spec('yaml') is None, 'fixture must have no PyYAML'
'''
        return subprocess.run([sys.executable,'-I','-S','-c',prelude+textwrap.dedent(program)],capture_output=True,text=True)

    def test_all_release_helpers_import_without_pyyaml(self):
        result=self.bare('''
            import importlib
            from pathlib import Path
            root=Path(sys.path[0])
            modules=sorted(p.stem for p in root.glob('*.py') if not p.name.startswith('test_') and p.stem!='approval_summary')
            assert len(modules)>=14
            for name in modules: importlib.import_module(name)
            assert 'yaml' not in sys.modules
            print('stdlib helper imports:',','.join(modules))
        ''')
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)

    def test_installer_fixture_builds_without_pyyaml(self):
        result=self.bare('''
            import gate,make_installer_fixture,tempfile
            from pathlib import Path
            with tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary);pristine=root/'installer.sh';output=root/'fixture'
                pristine.write_text('\\n'.join('"hennery-'+target+'.tar.xz")\\n    ;;' for target in sorted(gate.TARGETS)))
                make_installer_fixture.make(pristine,output)
                assert (output/'hennery-installer.sh').is_file()
                assert len(list(output.glob('*.tar.xz')))==2*len(gate.TARGETS)+1
        ''')
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
