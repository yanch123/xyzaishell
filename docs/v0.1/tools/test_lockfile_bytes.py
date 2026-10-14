"""Cross-platform parsing and checkout-byte regressions; no dependencies needed."""
from pathlib import Path
import subprocess
import tempfile
import unittest

import dependency_inventory as inventory


LOCK = b'''version = 4

[[package]]
name = "fixture"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0123456789abcdef"

[[package]]
name = "workspace"
version = "0.1.0"
'''


class LockfileByteTests(unittest.TestCase):
    def test_lf_crlf_and_mixed_line_endings_parse_identically(self):
        expected = {('fixture', '1.2.3', 'registry+https://github.com/rust-lang/crates.io-index'): '0123456789abcdef',
                    ('workspace', '0.1.0', None): None}
        crlf = LOCK.replace(b'\n', b'\r\n')
        mixed = LOCK.replace(b'\n', b'\r\n', 5)
        for data in (LOCK, crlf, mixed):
            before = inventory.sha(data)
            self.assertEqual(inventory.lock_checksums(data), expected)
            self.assertEqual(inventory.sha(data), before)
        # Parsing is tolerant; provenance must still detect byte differences.
        self.assertNotEqual(inventory.sha(LOCK), inventory.sha(crlf))

    def test_missing_and_duplicate_packages_fail_closed(self):
        for data in (b'version = 4\n', b'[[package]]\nversion = "1"\n', LOCK + LOCK):
            with self.subTest(data=data), self.assertRaises(ValueError):
                inventory.lock_checksums(data)

    def test_windows_autocrlf_checkout_preserves_source_and_exact_evidence(self):
        attributes = Path(__file__).resolve().parents[3] / '.gitattributes'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-c', 'user.name=Fixture', '-c',
                                                'user.email=fixture@example.invalid', *args], cwd=root)
            git('init', '--quiet')
            git('config', 'core.autocrlf', 'false')
            files = {'.gitattributes': attributes.read_bytes(), 'Cargo.lock': LOCK,
                     'package-lock.json': b'{"fixture": true}\n',
                     'fixture.ps1': b'Write-Output "fixture"\n',
                     'fixture.nsi': b'Name "fixture"\n',
                     'fixture.png': b'\x89PNG\r\n\x1a\n\x00binary\r\n',
                     'docs/v0.1/tools/license-overrides/vendor.txt': b'Exact upstream notice\r\n'}
            for name, data in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            git('add', '.')
            git('commit', '--quiet', '-m', 'byte fixture')
            git('config', 'core.autocrlf', 'true')
            for name in files:
                (root / name).unlink()
            git('checkout', '--', '.')
            for name, data in files.items():
                self.assertEqual((root / name).read_bytes(), data, name)


if __name__ == '__main__':
    unittest.main()
