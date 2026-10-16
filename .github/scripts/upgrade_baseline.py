#!/usr/bin/env python3
"""Fetch/verify the digest-pinned public v0.1.3 prerelease upgrade baseline; never install or publish."""
import argparse
from pathlib import Path
import urllib.request

import artifacts

BASE_VERSION = "0.1.3"
TARGET_VERSION = "0.1.4"
BASE_COMMIT = "00660aa73474827cbd78e6f821c3e2b9a254dad2"
BASE_RUN = "38841350614"
RELEASE_URL = "https://github.com/yanch123/xyzaishell/releases/download/v0.1.3/"
# Public prerelease v0.1.3 (release 414487406), reverified from public bytes
# on 2026-10-16. Both application hashes are independently extracted from these
# exact public packages, not earlier same-source candidate CI artifacts.
# See fixtures/upgrade-v0.1.3.json. The upstream release is not immutable:
# fail-closed pins bind the baseline; changes require a reviewed update.
PINS = {
    "linux-x64": {
        "package": ("xyzaishell-0.1.3-linux-x64.deb", 12090408, "8067f1c963a8427d3839738baf192a51f410ca192499a98880ca62148e0230e1"),
        "manifest": ("build-linux-x64.json", 1107, "d708c8deec66ee796ae92b6556f37d4a40da8a8f56c75b86822256ef5db8b700"),
        "application_sha256": "c9baca85048dfa4f8b5b1ece639e0634b0c454ad243299a9e647f25057b03028",
    },
    "windows-x64": {
        "package": ("xyzaishell-0.1.3-windows-x64.exe", 6597258, "d6e80d6da81aca9ab451bf8ff4f71b930e544272a77ae7861e4b77b67ecf3894"),
        "manifest": ("build-windows-x64.json", 1132, "e85105cb41bf5fcd1d995e9d0517e14ed1ab0400feed9b336d111b0581dd4b47"),
        "application_sha256": "45c43d393089529574d4e9c71f4403ab378fe6ec6dbb9a0f3b8277d9eb685e49",
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
