"""Run with python -m unittest discover -s docs/v0.1/tools -p 'test_*.py'."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parents[2]


class ThirdPartyTests(unittest.TestCase):
    def test_override_integrity(self):
        base = TOOLS / 'license-overrides'
        records = json.loads((base / 'sources.json').read_text(encoding='utf-8'))
        self.assertEqual(len(records), len({(p['name'], p['version']) for p in records}))
        for package in records:
            self.assertEqual(hashlib.sha256((base / package['file']).read_bytes()).hexdigest(), package['sha256'])
            self.assertTrue(package['url'].startswith('https://'))
            self.assertTrue(package['crate_vcs']['git']['sha1'])

    def test_unknown_output_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            sentinel = Path(directory) / 'do-not-remove.txt'
            sentinel.write_text('kept')
            result = subprocess.run([sys.executable, str(TOOLS / 'prepare_third_party.py'),
                                     '--offline', '--output', directory], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(sentinel.read_text(), 'kept')

    def test_two_platforms_and_determinism(self):
        with tempfile.TemporaryDirectory() as directory:
            for target in ['x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc']:
                output = Path(directory) / target
                command = [sys.executable, str(TOOLS / 'prepare_third_party.py'), '--offline',
                           '--target', target, '--output', str(output)]
                subprocess.run(command, cwd=ROOT, check=True)
                snapshot = {p.relative_to(output): p.read_bytes() for p in output.rglob('*') if p.is_file()}
                subprocess.run(command, cwd=ROOT, check=True)
                self.assertEqual(snapshot, {p.relative_to(output): p.read_bytes() for p in output.rglob('*') if p.is_file()})
                inventory = json.loads((output / 'runtime-inventory.json').read_text(encoding='utf-8'))
                self.assertEqual(inventory['target'], target)
                if 'windows' in target:
                    self.assertEqual(inventory['vendor_components'][0]['version'], '1.0.3800.47')
                    self.assertIn('NOTICES AND INFORMATION', (output / 'THIRD-PARTY-NOTICES.txt').read_text(encoding='utf-8'))
                else:
                    self.assertEqual(inventory['vendor_components'], [])
                names = {p['name'] for p in inventory['packages']}
                self.assertNotIn('playwright', names)
                self.assertNotIn('selectors', names)
                self.assertNotIn('cssparser', names)
                for package in inventory['packages']:
                    self.assertTrue(package['files'] or package.get('supplemental_license'))
                    if 'MPL-2.0' in package['license_declared']:
                        source = output / package['source_archive']['path']
                        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), package['lock_checksum'])


if __name__ == '__main__':
    unittest.main()
