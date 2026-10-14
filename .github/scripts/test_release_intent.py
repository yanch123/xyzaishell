import json
import subprocess
import tempfile
import unittest
from pathlib import Path

import release_intent as intent


class ReleaseIntentTests(unittest.TestCase):
    def valid(self, mode="draft"):
        sha = "a" * 40
        return {"schema_version": 1, "tag": "v0.1.0", "source_commit": sha, "mode": mode,
                "confirmation": f"{mode} xyzaishell v0.1.0 from {sha}",
                "acceptance": {key: {"status": "passed", "evidence": ["https://github.com/owner/repo/issues/1"],
                                     "notes": "Synthetic test record"} for key in intent.REQUIRED_EVIDENCE}}

    def test_valid_publish_and_first_release_upgrade_exception(self):
        value = self.valid("publish")
        value["acceptance"]["version_upgrade"] = {"status": "not_applicable", "evidence": [], "notes": "First release; no older released version exists."}
        self.assertEqual(intent.validate(value), value)

    def test_draft_keeps_pending_records_honest(self):
        value = self.valid()
        value["acceptance"]["windows_10"] = {"status": "pending", "evidence": [], "notes": "Not yet tested"}
        self.assertIn("windows_10: pending", intent.notes(intent.validate(value), "https://github.com/a/b/actions/runs/123"))

    def test_pending_acceptance_blocks_publish(self):
        for key in intent.REQUIRED_EVIDENCE:
            value = self.valid("publish")
            value["acceptance"][key]["status"] = "pending"
            with self.assertRaises(ValueError):
                intent.validate(value)

    def test_prerelease_only_allows_pending_desktop_acceptance(self):
        for key in intent.REQUIRED_EVIDENCE:
            value = self.valid("prerelease")
            value["acceptance"][key] = {"status": "pending", "evidence": [], "notes": "Not yet tested"}
            if key in intent.DESKTOP_EVIDENCE:
                self.assertEqual(intent.validate(value), value)
                rendered = intent.notes(value, "https://github.com/a/b/actions/runs/123")
                self.assertIn(f"{key}: pending", rendered)
                self.assertIn("预发布试用版", rendered)
                self.assertIn("不作为正式稳定版或 Latest", rendered)
            else:
                with self.assertRaises(ValueError):
                    intent.validate(value)

    def test_prerelease_cannot_reuse_publish_confirmation_or_waive_desktop(self):
        value = self.valid("publish")
        value["mode"] = "prerelease"
        with self.assertRaises(ValueError):
            intent.validate(value)
        value = self.valid("prerelease")
        value["acceptance"]["windows_10"]["status"] = "not_applicable"
        with self.assertRaises(ValueError):
            intent.validate(value)

    def test_no_waiver_for_required_platforms(self):
        value = self.valid("publish")
        value["acceptance"]["windows_11"]["status"] = "not_applicable"
        with self.assertRaises(ValueError):
            intent.validate(value)

    def test_mismatched_confirmation_and_unknown_fields_rejected(self):
        for field, replacement in [("confirmation", "publish"), ("tag", "v0.1.0\nsha=evil"), ("source_commit", "main")]:
            value = self.valid()
            value[field] = replacement
            with self.assertRaises(ValueError):
                intent.validate(value)
        value = self.valid()
        value["allow_incomplete"] = True
        with self.assertRaises(ValueError):
            intent.validate(value)

    def test_missing_evidence_and_credential_urls_rejected(self):
        for links in [[], ["https://token@example.com/proof"], ["file:///tmp/proof"], ["https://example.com/has space"]]:
            value = self.valid()
            value["acceptance"]["dependency_notices"]["evidence"] = links
            with self.assertRaises(ValueError):
                intent.validate(value)

    def test_duplicate_keys_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "intent.json"
            path.write_text('{"mode":"draft","mode":"publish"}', encoding="utf-8")
            with self.assertRaises(ValueError):
                intent.read(path)

    def test_read_roundtrip_and_size_limit(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "intent.json"
            path.write_text(json.dumps(self.valid()), encoding="utf-8")
            self.assertEqual(intent.read(path), self.valid())
            path.write_text(" " * 65537, encoding="utf-8")
            with self.assertRaises(ValueError):
                intent.read(path)

    def test_activation_only_allows_release_control_and_evidence(self):
        for change in ("evidence", "application", "workflow", "rename_application", "symlink_evidence", "symlink_intent", "unchanged_intent", "foreign_ancestor"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                def git(*args):
                    return subprocess.check_output(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args], cwd=root, text=True).strip()
                git("init", "--quiet")
                (root / "src").mkdir()
                (root / "src/app.rs").write_text("original", encoding="utf-8")
                git("add", ".")
                git("commit", "--quiet", "-m", "candidate")
                source = git("rev-parse", "HEAD")
                if change == "foreign_ancestor":
                    git("checkout", "--quiet", "--orphan", "unrelated")
                (root / ".github").mkdir()
                request = root / ".github/release-intent.json"
                if change == "symlink_intent":
                    request.symlink_to("../src/app.rs")
                elif change != "unchanged_intent":
                    request.write_text("{}", encoding="utf-8")
                evidence = root / "docs/v0.1/release-evidence"
                evidence.mkdir(parents=True)
                if change in ("evidence", "unchanged_intent", "foreign_ancestor"):
                    (evidence / "test.md").write_text("Evidence", encoding="utf-8")
                elif change == "application":
                    (root / "src/app.rs").write_text("untested", encoding="utf-8")
                elif change == "workflow":
                    (root / ".github/ci.yml").write_text("untested", encoding="utf-8")
                elif change == "rename_application":
                    (root / "src/app.rs").rename(evidence / "renamed.rs")
                elif change == "symlink_evidence":
                    (evidence / "test.md").symlink_to("../../../src/app.rs")
                git("add", "-A")
                git("commit", "--quiet", "-m", "activation")
                activation = git("rev-parse", "HEAD")
                if change == "evidence":
                    intent.verify_activation_tree(root, source, activation)
                else:
                    with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                        intent.verify_activation_tree(root, source, activation)


if __name__ == "__main__":
    unittest.main()
