"""Fixture integrity checks, not a substitute for real Bash/OpenSSH acceptance."""
import pathlib
import re
import subprocess
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent


class Bash42FixtureTests(unittest.TestCase):
    def test_manifest_covers_archive_and_exactly_first_46_official_patches(self):
        lines = (SCRIPTS / 'fixtures/bash-4.2.46/SHA256SUMS').read_text().splitlines()
        expected = ['bash-4.2.tar.gz'] + [f'patches/bash42-{n:03}' for n in range(1, 47)]
        self.assertEqual(len(lines), len(expected))
        for line, filename in zip(lines, expected):
            self.assertRegex(line, r'^[0-9a-f]{64}  ' + re.escape(filename) + '$')
        self.assertEqual(len({line.split()[0] for line in lines}), 47)

    def test_builder_cannot_overwrite_existing_directory(self):
        with tempfile.TemporaryDirectory() as root:
            sentinel = pathlib.Path(root) / 'keep'
            sentinel.write_text('existing data')
            result = subprocess.run(['bash', str(SCRIPTS / 'build-bash-4.2.sh'), root],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('must not already exist', result.stderr)
            self.assertEqual(sentinel.read_text(), 'existing data')

    def test_scripts_parse(self):
        for script in ['build-bash-4.2.sh', 'openssh-test.sh']:
            subprocess.run(['bash', '-n', str(SCRIPTS / script)], check=True)


if __name__ == '__main__':
    unittest.main()
