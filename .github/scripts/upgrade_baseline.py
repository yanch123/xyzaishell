#!/usr/bin/env python3
"""Fetch/verify the digest-pinned public v0.1.4 prerelease upgrade baseline; never install or publish."""
import argparse
from pathlib import Path
import urllib.request

import artifacts

BASE_VERSION = "0.1.4"
TARGET_VERSION = "0.2.0"
BASE_COMMIT = "0e29b1e5912f35e9eabd73ac073524af143cd622"
BASE_RUN = "38937958608"
RELEASE_URL = "https://github.com/yanch123/xyzaishell/releases/download/v0.1.4/"
# Public prerelease v0.1.4 (release 415214180), reverified from public bytes
# on 2026-10-16. Both application hashes are independently extracted from these
# exact public packages, not earlier same-source candidate CI artifacts.
# See fixtures/upgrade-v0.1.4.json. The upstream release is not immutable:
# fail-closed pins bind the baseline; changes require a reviewed update.
PINS = {'linux-x64': {'package': ('xyzaishell-0.1.4-linux-x64.deb',
                           12161974,
                           '8546ae17e2aeca0889285af197954eb7201e1d87972e18ac47bd810b69d297d8'),
               'manifest': ('build-linux-x64.json',
                            1107,
                            'ebc6fb6c6cc93b550d78a72d20129e5a7dd3507c530c93e4b5d866536f30e2a3'),
               'application_sha256': '0b8d6a20927b36f2c9a09015330fd16558e652ba98a19bee220c2863df756c5b'},
 'windows-x64': {'package': ('xyzaishell-0.1.4-windows-x64.exe',
                             6675716,
                             '8761a106a6c40c34d3c34de1f8640ab77e0a5e12945d438b3812f12a2864756e'),
                 'manifest': ('build-windows-x64.json',
                              1132,
                              '0e217c42e655a57098df5e3313fe62ac92a8d15739caaff3844b1cfb9532326b'),
                 'application_sha256': '8575b363fc3a7cb06291a5cfc48714c2d1b9fba1e399e4b90036d30ae30fc3e7'}}


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
