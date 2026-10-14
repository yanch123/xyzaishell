#!/usr/bin/env python3
"""Fetch/verify the digest-pinned public v0.1.1 prerelease upgrade baseline; never install or publish."""
import argparse
from pathlib import Path
import urllib.request

import artifacts

BASE_VERSION = "0.1.1"
TARGET_VERSION = "0.1.2"
BASE_COMMIT = "18eba0cf02a2c265862e957ba86f1cb24e4bedde"
BASE_RUN = "38561359264"
RELEASE_URL = "https://github.com/yanch123/xyzaishell/releases/download/v0.1.1/"
# Public prerelease v0.1.1 (release 412688104), reverified against the GitHub
# release/tag API on 2026-10-14. All asset sizes/SHA256 match the public release.
# Installed binary hashes match the exact-source native acceptance runs; the
# Linux hash also matches an independently extracted DEB. The upstream release
# is not immutable: these fail-closed pins, not the tag alone, bind the baseline.
# Changing the baseline requires a reviewed update.
PINS = {
    "linux-x64": {
        "package": ("xyzaishell-0.1.1-linux-x64.deb", 11674174, "801bcd2690531f5e97e6e7d0b4942bf47e003e6e74aaea0b410c8d349fa34d2f"),
        "manifest": ("build-linux-x64.json", 1107, "b0e28f5ce58a6fac050c0d1db816edec08e949f437f9055a97ecbfbb953facc0"),
        "application_sha256": "c791c94868c68ff280a279170c3ec70a0e352a152d727e638b40f78b330e931b",
    },
    "windows-x64": {
        "package": ("xyzaishell-0.1.1-windows-x64.exe", 6410915, "4b184c98334be48fe8018087c6fe579cea9cfaac905285fc74030579034e7ed7"),
        "manifest": ("build-windows-x64.json", 1132, "052fd6f450cc08190c27444298163c762e3a56a5fb399c19ce2dbabf5521d5e1"),
        "application_sha256": "369b98e1fab0c1ff8b3d8cea88224249b121914945f65dc21cdf19f50817f013",
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
                and artifacts.sha256(application) == pins["application_sha256"], "Installed baseline executable differs from verified v0.1.1")
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
