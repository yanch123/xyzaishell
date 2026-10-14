import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import release_intent as intent


class ReleaseIntentTests(unittest.TestCase):
    def valid(self, mode="draft", tag="v0.1.0"):
        sha = "a" * 40
        return {"schema_version": 1, "tag": tag, "source_commit": sha, "mode": mode,
                "confirmation": f"{mode} xyzaishell {tag} from {sha}",
                "acceptance": {key: {"status": "passed", "evidence": ["https://github.com/owner/repo/issues/1"],
                                     "notes": "Synthetic test record"} for key in intent.REQUIRED_EVIDENCE}}

    def test_valid_publish_and_first_release_upgrade_exception(self):
        value = self.valid("publish")
        value["acceptance"]["version_upgrade"] = {"status": "not_applicable", "evidence": [], "notes": "First release; no older released version exists."}
        self.assertEqual(intent.validate(value), value)

    def test_v011_upgrade_requires_passed_evidence_before_release(self):
        for mode in ("prerelease", "publish"):
            with self.subTest(mode=mode, status="passed"):
                value = self.valid(mode, "v0.1.1")
                self.assertEqual(intent.validate(value), value)
            for status in ("not_applicable", "pending"):
                with self.subTest(mode=mode, status=status):
                    value = self.valid(mode, "v0.1.1")
                    value["acceptance"]["version_upgrade"] = {
                        "status": status, "evidence": [], "notes": "Upgrade not yet tested"}
                    with self.assertRaises(ValueError):
                        intent.validate(value)
            with self.subTest(mode=mode, status="passed without evidence"):
                value = self.valid(mode, "v0.1.1")
                value["acceptance"]["version_upgrade"]["evidence"] = []
                with self.assertRaises(ValueError):
                    intent.validate(value)

    def test_v011_draft_may_keep_upgrade_pending_but_cannot_waive_it(self):
        value = self.valid("draft", "v0.1.1")
        value["acceptance"]["version_upgrade"] = {
            "status": "pending", "evidence": [], "notes": "Upgrade not yet tested"}
        self.assertEqual(intent.validate(value), value)
        value["acceptance"]["version_upgrade"]["status"] = "not_applicable"
        with self.assertRaises(ValueError):
            intent.validate(value)

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

    def activation(self, root, tag, evidence_path, change="evidence"):
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
            value = self.valid(tag=tag)
            value["source_commit"] = source
            value["confirmation"] = f"draft xyzaishell {tag} from {source}"
            request.write_text(json.dumps(value), encoding="utf-8")
        evidence = root / evidence_path
        evidence.parent.mkdir(parents=True)
        if change in ("evidence", "unchanged_intent", "foreign_ancestor", "executable_evidence", "executable_intent"):
            evidence.write_text("Evidence", encoding="utf-8")
        elif change == "application":
            (root / "src/app.rs").write_text("untested", encoding="utf-8")
        elif change == "workflow":
            (root / ".github/workflows").mkdir()
            (root / ".github/workflows/ci.yml").write_text("untested", encoding="utf-8")
        elif change == "rename_application":
            (root / "src/app.rs").rename(evidence)
        elif change == "symlink_evidence":
            evidence.symlink_to("../../../src/app.rs")
        git("add", "-A")
        if change == "executable_evidence":
            git("update-index", "--chmod=+x", evidence_path)
        elif change == "executable_intent":
            git("update-index", "--chmod=+x", ".github/release-intent.json")
        git("commit", "--quiet", "-m", "activation")
        return source, git("rev-parse", "HEAD")

    def test_activation_only_allows_release_control_and_evidence(self):
        for tag, directory_version in (("v0.1.0", "v0.1"), ("v0.1.1", "v0.1.1")):
            for change in ("evidence", "application", "workflow", "rename_application", "symlink_evidence", "symlink_intent", "executable_evidence", "executable_intent", "unchanged_intent", "foreign_ancestor"):
                with self.subTest(tag=tag, change=change), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    source, activation = self.activation(root, tag, f"docs/{directory_version}/release-evidence/test.md", change)
                    if change == "evidence":
                        intent.verify_activation_tree(root, source, activation, tag)
                    else:
                        with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                            intent.verify_activation_tree(root, source, activation, tag)

    def test_activation_evidence_is_scoped_to_the_intended_version(self):
        for tag, allowed_path in (("v0.1.0", "docs/v0.1/release-evidence/test.md"),
                                  ("v0.1.1", "docs/v0.1.1/release-evidence/test.md")):
            for path in ("docs/v0.1/release-evidence/test.md", "docs/v0.1.0/release-evidence/test.md",
                         "docs/v0.1.1/release-evidence/test.md", "docs/v0.1.2/release-evidence/test.md",
                         "docs/v0.1.1/release-evidence-other/test.md", "docs/v0.1.1/release-evidence.md",
                         "docs/v0.1.1/guide.md"):
                with self.subTest(tag=tag, path=path), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    source, activation = self.activation(root, tag, path)
                    if path == allowed_path:
                        intent.verify_activation_tree(root, source, activation, tag)
                    else:
                        with self.assertRaisesRegex(ValueError, "Untested change"):
                            intent.verify_activation_tree(root, source, activation, tag)

    def test_activation_requires_stable_tag_and_exact_shas(self):
        with tempfile.TemporaryDirectory() as directory:
            for tag in (None, "", "v0.1.1/../v0.1", "v0.1.1-rc1", "v0.1.1\n"):
                with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, "Stable vX.Y.Z tag required"):
                    intent.verify_activation_tree(directory, "a" * 40, "b" * 40, tag)
            for sha in ("main", "a" * 39, "A" * 40, "a" * 40 + "\n"):
                for source, activation in ((sha, "b" * 40), ("a" * 40, sha)):
                    with self.subTest(source=source, activation=activation), self.assertRaisesRegex(ValueError, "Exact source and activation SHAs required"):
                        intent.verify_activation_tree(directory, source, activation, "v0.1.1")

    def test_cli_uses_validated_intent_tag_for_activation_boundary(self):
        for path, accepted in (("docs/v0.1.1/release-evidence/test.md", True),
                               ("docs/v0.1/release-evidence/test.md", False)):
            with self.subTest(path=path), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                _, activation = self.activation(root, "v0.1.1", path)
                result = subprocess.run(
                    [sys.executable, str(Path(intent.__file__).resolve()),
                     ".github/release-intent.json", "--activation-sha", activation],
                    cwd=root, text=True, capture_output=True)
                if accepted:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIn("Validated draft intent for v0.1.1", result.stdout)
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Untested change", result.stderr)


if __name__ == "__main__":
    unittest.main()
