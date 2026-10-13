import json
from pathlib import Path
import tempfile
import subprocess
import zipfile
import unittest
from unittest.mock import patch

import artifacts


class ArtifactTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        (self.root / "src-tauri").mkdir()
        for name in ("package.json", "package-lock.json", "src-tauri/tauri.conf.json"):
            (self.root / name).write_text(json.dumps({"version": "0.1.0"}), encoding="utf-8")
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = []\n[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n[workspace.dependencies]\n', encoding="utf-8")
        (self.root / "Cargo.lock").write_text("# Test fixture only\n", encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def installer(self, platform="linux-x64"):
        bundle, extension = artifacts.PLATFORMS[platform]
        directory = self.root / "target/release/bundle" / bundle
        directory.mkdir(parents=True)
        (directory / f"app{extension}").write_bytes(b"not a real installer; test fixture")
        output = self.root / f"output-{platform}"
        with patch.object(artifacts, "command", return_value="a" * 40):
            artifacts.collect(self.root, output, platform, bundle)
        return output, artifacts.read_json(output / f"build-{platform}.json")

    def test_matching_versions_and_tag(self):
        self.assertEqual(artifacts.check_version(self.root, "v0.1.0"), "0.1.0")

    def test_mismatched_tag_rejected(self):
        for tag in ("dev", "v0.2.0", "v0.1.0;echo bad"):
            with self.assertRaises(ValueError):
                artifacts.check_version(self.root, tag)

    def test_mismatched_manifest_version_rejected(self):
        (self.root / "src-tauri/tauri.conf.json").write_text('{"version":"0.2.0"}', encoding="utf-8")
        with self.assertRaises(ValueError):
            artifacts.check_version(self.root)

    def test_missing_cargo_workspace_version_rejected(self):
        (self.root / "Cargo.toml").write_text('[package]\nversion = "0.1.0"\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            artifacts.check_version(self.root)

    def test_collect_both_expected_installer_formats(self):
        for platform in artifacts.PLATFORMS:
            output, manifest = self.installer(platform)
            path = artifacts.validate_manifest(manifest, platform, "0.1.0", "a" * 40, self.root, output)
            self.assertTrue(path.exists())
            checksums = (output / f"SHA256SUMS-{platform}.txt").read_text(encoding="utf-8")
            self.assertIn(artifacts.sha256(path), checksums)
            self.assertIn(f"build-{platform}.json", checksums)

    def test_missing_installer_rejected(self):
        with self.assertRaises(ValueError):
            artifacts.collect(self.root, self.root / "output", "linux-x64", "deb")

    def test_wrong_platform_bundle_rejected(self):
        with self.assertRaises(ValueError):
            artifacts.collect(self.root, self.root / "output", "windows-x64", "deb")

    def test_multiple_installers_rejected(self):
        output, _ = self.installer()
        (self.root / "target/release/bundle/deb/extra.deb").write_bytes(b"ambiguous")
        with self.assertRaises(ValueError):
            artifacts.collect(self.root, output, "linux-x64", "deb")

    def test_changed_installer_rejected(self):
        output, manifest = self.installer()
        (output / manifest["package_file"]).write_bytes(b"tampered")
        with self.assertRaises(ValueError):
            artifacts.validate_manifest(manifest, "linux-x64", "0.1.0", "a" * 40, self.root, output)

    def test_wrong_source_commit_rejected(self):
        output, manifest = self.installer()
        with self.assertRaises(ValueError):
            artifacts.validate_manifest(manifest, "linux-x64", "0.1.0", "b" * 40, self.root, output)

    def test_changed_lockfile_rejected(self):
        output, manifest = self.installer()
        (self.root / "Cargo.lock").write_text("changed", encoding="utf-8")
        with self.assertRaises(ValueError):
            artifacts.validate_manifest(manifest, "linux-x64", "0.1.0", "a" * 40, self.root, output)

    def test_release_archives_and_all_checksums(self):
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        subprocess.run(["git", "add", "Cargo.toml", "Cargo.lock", "package.json", "package-lock.json", "src-tauri/tauri.conf.json"], cwd=self.root, check=True)
        subprocess.run(["git", "-c", "user.name=Pipeline Test", "-c", "user.email=pipeline@example.invalid", "commit", "--quiet", "-m", "Fixture"], cwd=self.root, check=True)
        commit = artifacts.command("git", "rev-parse", "HEAD", cwd=self.root)
        for platform in artifacts.PLATFORMS:
            output, manifest = self.installer(platform)
            manifest["source_commit"] = commit
            artifacts.write_json(output / f"build-{platform}.json", manifest)
        destination = self.root / "release-output"
        artifacts.release(self.root, self.root, destination, "v0.1.0")
        self.assertEqual(len(list(destination.iterdir())), 7)
        with zipfile.ZipFile(destination / "xyzaishell-0.1.0-source.zip") as source:
            self.assertIn("xyzaishell-0.1.0/Cargo.lock", source.namelist())
            self.assertFalse(any("output-" in name for name in source.namelist()))
        for line in (destination / "SHA256SUMS.txt").read_text(encoding="utf-8").splitlines():
            digest, filename = line.split("  ", 1)
            self.assertEqual(artifacts.sha256(destination / filename), digest)

    def test_path_traversal_in_manifest_rejected(self):
        output, manifest = self.installer()
        manifest["package_file"] = "../untrusted.deb"
        with self.assertRaises(ValueError):
            artifacts.validate_manifest(manifest, "linux-x64", "0.1.0", "a" * 40, self.root, output)


if __name__ == "__main__":
    unittest.main()
