import copy
import hashlib
import tempfile
import unittest
from pathlib import Path

import verify_release as release


class VerifyReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.intent = {"mode": "prerelease", "tag": "v0.1.0", "source_commit": "a" * 40}
        assets = []
        for name in sorted(release.asset_names(self.intent["tag"])):
            data = (name + " synthetic test only").encode()
            (self.root / name).write_bytes(data)
            assets.append({"name": name, "size": len(data), "state": "uploaded", "digest": "sha256:" + hashlib.sha256(data).hexdigest()})
        self.remote = {"id": 123, "tag_name": "v0.1.0", "draft": False, "prerelease": True, "assets": assets}
        self.summary = [{"tagName": "v0.1.0", "isDraft": False, "isPrerelease": True, "isLatest": False}]
        self.ref = {"ref": "refs/tags/v0.1.0", "object": {"type": "commit", "sha": "a" * 40}}

    def verify(self, draft_stage=False):
        return release.verify(self.intent, self.remote, self.summary, self.ref, self.root, draft_stage)

    def test_exact_prerelease_and_all_seven_assets(self):
        self.assertEqual(self.verify(), "prerelease")

    def test_draft_upload_checkpoint_before_any_publication(self):
        self.remote.update(draft=True, prerelease=False)
        self.summary[0].update(isDraft=True, isPrerelease=False)
        self.assertEqual(self.verify(draft_stage=True), "draft")
        with self.assertRaises(ValueError):
            self.verify()

    def test_stable_publish_requires_stable_latest_flags(self):
        self.intent["mode"] = "publish"
        with self.assertRaises(ValueError):
            self.verify()
        self.remote["prerelease"] = False
        self.summary[0].update(isPrerelease=False, isLatest=True)
        self.assertEqual(self.verify(), "publish")

    def test_wrong_flags_rejected(self):
        for key in ("isDraft", "isPrerelease", "isLatest"):
            with self.subTest(key=key):
                original = self.summary[0][key]
                self.summary[0][key] = not original
                with self.assertRaises(ValueError):
                    self.verify()
                self.summary[0][key] = original
        self.remote["prerelease"] = False
        with self.assertRaises(ValueError):
            self.verify()

    def test_wrong_tag_or_source_rejected(self):
        for key, value in (("sha", "b" * 40), ("type", "tag")):
            original = self.ref["object"][key]
            self.ref["object"][key] = value
            with self.assertRaises(ValueError):
                self.verify()
            self.ref["object"][key] = original
        self.remote["tag_name"] = "v0.2.0"
        with self.assertRaises(ValueError):
            self.verify()

    def test_missing_duplicate_extra_or_incomplete_asset_rejected(self):
        baseline = copy.deepcopy(self.remote["assets"])
        for variant in (baseline[:-1], baseline + [baseline[0]], baseline[:-1] + [baseline[0]]):
            self.remote["assets"] = variant
            with self.assertRaises(ValueError):
                self.verify()
        for field, replacement in (("state", "starter"), ("size", 0), ("digest", None), ("digest", "sha256:" + "0" * 64)):
            self.remote["assets"] = copy.deepcopy(baseline)
            self.remote["assets"][0][field] = replacement
            with self.assertRaises(ValueError):
                self.verify()

    def test_missing_local_installer_and_changed_local_bytes_rejected(self):
        path = self.root / "xyzaishell-0.1.0-windows-x64.exe"
        path.write_bytes(b"different")
        with self.assertRaises(ValueError):
            self.verify()
        path.unlink()
        with self.assertRaises(ValueError):
            self.verify()

    def test_missing_or_ambiguous_summary_rejected(self):
        for summary in ([], self.summary * 2):
            with self.assertRaises(ValueError):
                release.verify(self.intent, self.remote, summary, self.ref, self.root)


if __name__ == "__main__":
    unittest.main()
