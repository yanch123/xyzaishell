#!/usr/bin/env python3
"""Fetch/verify the digest-pinned public v0.1.2 prerelease upgrade baseline; never install or publish."""
import argparse
from pathlib import Path
import urllib.request

import artifacts

BASE_VERSION = "0.1.2"
TARGET_VERSION = "0.1.3"
BASE_COMMIT = "423fc24242ca6b168afc151d7b9f3985a92c1566"
BASE_RUN = "38594474625"
RELEASE_URL = "https://github.com/yanch123/xyzaishell/releases/download/v0.1.2/"
# Public prerelease v0.1.2 (release 412925486), reverified against the GitHub
# release/tag API on 2026-10-15. Downloaded package/manifest bytes from release
# run 38594474625 match every public asset size/SHA256. See fixtures/upgrade-v0.1.2.
# Installed binary hashes match the exact-source native acceptance runs; the
# Linux hash also matches an independently extracted DEB. The upstream release
# is not immutable: these fail-closed pins, not the tag alone, bind the baseline.
# Changing the baseline requires a reviewed update.
PINS = {
    "linux-x64": {
        "package": ("xyzaishell-0.1.2-linux-x64.deb", 11965316, "67a8323a3cbe19c5b876c2abe9a9a3cde69a2f03f7506525d1026f37c36ad984"),
        "manifest": ("build-linux-x64.json", 1107, "85f4cbbb26c3618ef3120113188768ced9a280a57b9efd7c3bd930e15118e600"),
        "application_sha256": "22421c99a4dc3984ae32421c35b24bb48f75decf584d16f23a89a47f7dc5d0c9",
    },
    "windows-x64": {
        "package": ("xyzaishell-0.1.2-windows-x64.exe", 6532549, "f1fdb819a23e30f767fabbc16ae916d4d8b254a978b980fb2ae624e5fa6a99c9"),
        "manifest": ("build-windows-x64.json", 1132, "334cd446aa04915d4c6f8b5e0f4801df88e921eba585effd6a7689f7cfdbabe4"),
        "application_sha256": "b22b65610ed7ea26bda03144249e7f40862d1e7b7b3463bcd0a170bcc2f9d62d",
    },
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


class HttpsRedirectOnly(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        require(newurl.startswith("https://"), "Refusing a non-HTTPS baseline redirect")
        return super().redirect_request(request, fp, code, message, headers, newurl)


def open_https(request, timeout):
    return urllib.request.build_opener(HttpsRedirectOnly()).open(request, timeout=timeout)


def validate_file(path, pin):
    name, size, digest = pin
    require(path.name == name and path.is_file() and not path.is_symlink(), "Missing regular baseline asset")
    require(path.stat().st_size == size and artifacts.sha256(path) == digest, f"Baseline byte mismatch: {name}")


def verify(directory, platform, target_version, application=None):
    require(target_version == TARGET_VERSION, "No reviewed upgrade baseline for this candidate version")
    directory = Path(directory)
    pins = PINS[platform]
    for key in ("package", "manifest"):
        validate_file(directory / pins[key][0], pins[key])
    manifest = artifacts.read_json(directory / pins["manifest"][0])
    expected = {"schema_version": 1, "source_commit": BASE_COMMIT, "version": BASE_VERSION,
                "platform": platform, "bundle": artifacts.PLATFORMS[platform][0],
                "package_file": pins["package"][0], "package_sha256": pins["package"][2],
                "workflow_run": BASE_RUN, "workflow_attempt": "1"}
    require(all(manifest.get(k) == v for k, v in expected.items()), "Baseline provenance differs from reviewed release")
    if application is not None:
        application = Path(application)
        require(application.is_file() and not application.is_symlink()
                and artifacts.sha256(application) == pins["application_sha256"], f"Installed baseline executable differs from verified v{BASE_VERSION}")
    return directory / pins["package"][0]


def download(directory, platform, target_version):
    require(target_version == TARGET_VERSION, "No reviewed upgrade baseline for this candidate version")
    directory = Path(directory)
    directory.mkdir(mode=0o700, parents=False, exist_ok=False)
    for key in ("package", "manifest"):
        name, size, _ = PINS[platform][key]
        request = urllib.request.Request(RELEASE_URL + name, headers={"User-Agent": "xyzaishell-upgrade-acceptance"})
        with open_https(request, timeout=90) as response:
            require(response.geturl().startswith("https://") and response.status == 200, "Expected HTTPS baseline download")
            body = response.read(size + 1)
        require(len(body) == size, "Unexpected baseline download length")
        (directory / name).write_bytes(body)
    return verify(directory, platform, target_version)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["download", "verify"])
    parser.add_argument("--platform", required=True, choices=PINS)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--application", type=Path)
    args = parser.parse_args()
    target = artifacts.check_version(Path.cwd())
    if args.action == "download":
        require(args.application is None, "Application verification is a separate installed-baseline step")
        result = download(args.directory, args.platform, target)
    else:
        result = verify(args.directory, args.platform, target, args.application)
    print(result.resolve())


if __name__ == "__main__":
    main()
