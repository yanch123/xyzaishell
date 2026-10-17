import base64
import copy
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import upgrade_baseline as baseline


class ReviewedReleasePinsTests(unittest.TestCase):
    def test_pins_match_reviewed_public_v014_bytes_and_release_run(self):
        evidence = json.loads((Path(__file__).parent / "fixtures/upgrade-v0.1.4.json").read_text())
        self.assertEqual((baseline.BASE_VERSION, baseline.TARGET_VERSION), ("0.1.4", "0.2.1"))
        self.assertEqual(baseline.BASE_COMMIT, evidence["source_commit"])
        self.assertEqual(baseline.BASE_RUN, evidence["workflow_run"])
        self.assertEqual(baseline.RELEASE_URL, "https://github.com/yanch123/xyzaishell/releases/download/v0.1.4/")
        for platform, pins in baseline.PINS.items():
            with self.subTest(platform=platform):
                reviewed = evidence["platforms"][platform]
                assets = {asset["name"]: asset for asset in reviewed["assets"]}
                for key in ("package", "manifest"):
                    name, size, digest = pins[key]
                    self.assertEqual((size, digest), (assets[name]["size"], assets[name]["sha256"]))
                    self.assertEqual(assets[name]["url"], baseline.RELEASE_URL + name)
                body = base64.b64decode(reviewed["manifest_bytes_base64"], validate=True)
                self.assertEqual((len(body), hashlib.sha256(body).hexdigest()), pins["manifest"][1:])
                manifest = json.loads(body)
                for key, value in {"source_commit": baseline.BASE_COMMIT, "version": baseline.BASE_VERSION,
                        "workflow_run": baseline.BASE_RUN, "workflow_attempt": "1", "platform": platform,
                        "package_file": pins["package"][0], "package_sha256": pins["package"][2]}.items():
                    self.assertEqual(manifest[key], value)
                self.assertEqual(reviewed["independently_extracted_application_sha256"], pins["application_sha256"])
                self.assertEqual(reviewed["extraction"]["package_sha256"], pins["package"][2])
                self.assertGreater(reviewed["extraction"]["application_size"], 0)


    def test_public_release_identity_is_not_candidate_ci_identity(self):
        self.assertEqual(baseline.BASE_RUN, "38937958608")
        self.assertEqual(baseline.PINS["windows-x64"]["package"][2], "8761a106a6c40c34d3c34de1f8640ab77e0a5e12945d438b3812f12a2864756e")
        self.assertEqual(baseline.PINS["windows-x64"]["application_sha256"], "8575b363fc3a7cb06291a5cfc48714c2d1b9fba1e399e4b90036d30ae30fc3e7")
        self.assertNotEqual(baseline.PINS["windows-x64"]["application_sha256"], "dc101bb4af086041fb1deac1c88de56a422f13ffd387ed33590191fc6af43217")


class UpgradeBaselineTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.pins = copy.deepcopy(baseline.PINS)
        self.contents = {}
        for platform, pin in self.pins.items():
            package = (platform + " synthetic installer").encode()
            digest = hashlib.sha256(package).hexdigest()
            pin["package"] = (pin["package"][0], len(package), digest)
            manifest = json.dumps({"schema_version": 1, "source_commit": baseline.BASE_COMMIT,
                "version": baseline.BASE_VERSION, "platform": platform, "bundle": baseline.artifacts.PLATFORMS[platform][0],
                "package_file": pin["package"][0], "package_sha256": digest,
                "workflow_run": baseline.BASE_RUN, "workflow_attempt": "1"}).encode()
            pin["manifest"] = (pin["manifest"][0], len(manifest), hashlib.sha256(manifest).hexdigest())
            pin["application_sha256"] = hashlib.sha256(b"baseline executable").hexdigest()
            self.contents[pin["package"][0]] = package
            self.contents[pin["manifest"][0]] = manifest
        self.patch = patch.object(baseline, "PINS", self.pins)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def fixture(self, platform):
        directory = self.root / platform
        directory.mkdir()
        for key in ("package", "manifest"):
            name = self.pins[platform][key][0]
            (directory / name).write_bytes(self.contents[name])
        return directory

    def test_both_exact_baselines_and_installed_executables(self):
        for platform in self.pins:
            directory = self.fixture(platform)
            app = self.root / (platform + "-app")
            app.write_bytes(b"baseline executable")
            self.assertEqual(baseline.verify(directory, platform, "0.2.1", app).name, self.pins[platform]["package"][0])
            app.write_bytes(b"other executable")
            with self.assertRaises(ValueError):
                baseline.verify(directory, platform, "0.2.1", app)

    def test_unknown_target_missing_or_changed_assets_fail(self):
        directory = self.fixture("linux-x64")
        for version in ("0.1.0", "0.1.1", "0.1.4", "0.1.5", "latest"):
            with self.assertRaises(ValueError):
                baseline.verify(directory, "linux-x64", version)
        package = directory / self.pins["linux-x64"]["package"][0]
        package.write_bytes(b"changed")
        with self.assertRaises(ValueError):
            baseline.verify(directory, "linux-x64", "0.2.1")
        package.unlink()
        with self.assertRaises(ValueError):
            baseline.verify(directory, "linux-x64", "0.2.1")

    def test_symlink_asset_rejected(self):
        directory = self.fixture("linux-x64")
        path = directory / self.pins["linux-x64"]["package"][0]
        target = self.root / "other"
        path.rename(target)
        path.symlink_to(target)
        with self.assertRaises(ValueError):
            baseline.verify(directory, "linux-x64", "0.2.1")

    def test_manifest_fields_are_bound_even_if_manifest_pin_is_updated(self):
        directory = self.fixture("linux-x64")
        path = directory / self.pins["linux-x64"]["manifest"][0]
        data = json.loads(path.read_text())
        for key in data:
            with self.subTest(key=key):
                changed = dict(data, **{key: "unreviewed"})
                body = json.dumps(changed).encode();path.write_bytes(body)
                self.pins["linux-x64"]["manifest"] = (path.name, len(body), hashlib.sha256(body).hexdigest())
                with self.assertRaises(ValueError):
                    baseline.verify(directory, "linux-x64", "0.2.1")

    def test_installed_executable_must_be_regular_and_not_a_symlink(self):
        directory = self.fixture("linux-x64")
        target = self.root / "real-application"
        target.write_bytes(b"baseline executable")
        application = self.root / "symlink-application"
        application.symlink_to(target)
        with self.assertRaises(ValueError):
            baseline.verify(directory, "linux-x64", "0.2.1", application)
        application.unlink()
        with self.assertRaises(ValueError):
            baseline.verify(directory, "linux-x64", "0.2.1", application)

    def test_download_is_bounded_and_never_overwrites_output(self):
        def response(request, timeout):
            self.assertEqual(timeout, 90)
            self.assertTrue(request.full_url.startswith(baseline.RELEASE_URL))
            name = request.full_url.rsplit('/', 1)[1]
            stream = io.BytesIO(self.contents[name])
            stream.status = 200;stream.geturl = lambda: request.full_url
            return stream
        directory = self.root / "downloaded"
        with patch.object(baseline, "open_https", side_effect=response) as opened:
            baseline.download(directory, "windows-x64", "0.2.1")
            self.assertEqual(opened.call_count, 2)
            with self.assertRaises(FileExistsError):
                baseline.download(directory, "windows-x64", "0.2.1")
            self.assertEqual(opened.call_count, 2)

    def test_http_redirect_is_rejected_before_following_it(self):
        request = baseline.urllib.request.Request(baseline.RELEASE_URL + "fixture")
        with self.assertRaises(ValueError):
            baseline.HttpsRedirectOnly().redirect_request(request, None, 302, "Found", {}, "http://invalid.example/file")

    def test_truncated_oversized_or_http_redirect_fails(self):
        for kind in ("short", "large", "http"):
            def response(request, timeout):
                body = self.contents[request.full_url.rsplit('/', 1)[1]]
                stream = io.BytesIO(body[:-1] if kind == "short" else body + b"x" if kind == "large" else body)
                stream.status = 200;stream.geturl = lambda: "http://invalid.example" if kind == "http" else request.full_url
                return stream
            with patch.object(baseline, "open_https", side_effect=response):
                with self.assertRaises(ValueError):
                    baseline.download(self.root / kind, "linux-x64", "0.2.1")


if __name__ == "__main__":
    unittest.main()
