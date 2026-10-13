#!/usr/bin/env python3
"""Package only expected installers and bind them to source/lockfile provenance."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

PLATFORMS = {"linux-x64": ("deb", ".deb"), "windows-x64": ("nsis", ".exe")}


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def command(*args, cwd=None):
    return subprocess.check_output(args, text=True, cwd=cwd).strip()


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def versions(root):
    root = Path(root)
    result = {
        "package.json": read_json(root / "package.json")["version"],
        "package-lock.json": read_json(root / "package-lock.json")["version"],
        "src-tauri/tauri.conf.json": read_json(root / "src-tauri/tauri.conf.json")["version"],
    }
    # The root workspace owns the inherited crate version. Deliberately fail
    # closed if that convention changes, rather than accepting an ambiguous value.
    cargo = (root / "Cargo.toml").read_text(encoding="utf-8")
    section = re.search(r"(?ms)^\[workspace\.package\]\s*\n(.*?)(?=^\[|\Z)", cargo)
    match = re.search(r'^version\s*=\s*"([^"]+)"\s*$', section[1], re.M) if section else None
    if not match:
        raise ValueError("Expected a literal version in [workspace.package]")
    result["Cargo.toml"] = match[1]
    return result


def check_version(root, tag=None):
    found = versions(root)
    expected = next(iter(found.values()))
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", expected):
        raise ValueError("A stable x.y.z version is required")
    if any(value != expected for value in found.values()):
        raise ValueError(f"Version mismatch: {found}")
    if tag is not None and tag != f"v{expected}":
        raise ValueError(f"Tag {tag!r} does not match v{expected}")
    return expected


def write_json(path, data):
    Path(path).write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def write_checksums(output, filename):
    output = Path(output)
    entries = [f"{sha256(path)}  {path.name}\n" for path in sorted(output.iterdir()) if path.is_file() and path.name != filename]
    if not entries:
        raise ValueError("No files to checksum")
    (output / filename).write_text("".join(entries), encoding="utf-8")


def collect(root, output, platform, bundle):
    root, output = Path(root).resolve(), Path(output).resolve()
    expected_bundle, extension = PLATFORMS[platform]
    if bundle != expected_bundle:
        raise ValueError("Platform and bundle do not match")
    version = check_version(root)
    candidates = list((root / "target/release/bundle" / bundle).glob(f"*{extension}"))
    if len(candidates) != 1 or not candidates[0].is_file() or candidates[0].is_symlink():
        raise ValueError(f"Expected exactly one regular {bundle} installer; found {len(candidates)}")
    output.mkdir(parents=True, exist_ok=False)
    filename = f"xyzaishell-{version}-{platform}{extension}"
    shutil.copyfile(candidates[0], output / filename)
    manifest = {
        "schema_version": 1,
        "source_commit": command("git", "rev-parse", "HEAD", cwd=root),
        "version": version,
        "platform": platform,
        "bundle": bundle,
        "package_file": filename,
        "package_sha256": sha256(output / filename),
        "lockfiles": {name: sha256(root / name) for name in ("Cargo.lock", "package-lock.json")},
        "rustc": command("rustc", "--version", "--verbose", cwd=root),
        "cargo": command("cargo", "--version", cwd=root),
        "node": command("node", "--version", cwd=root),
        "runner_os": os.environ.get("RUNNER_OS"),
        "runner_image": os.environ.get("ImageOS"),
        "runner_image_version": os.environ.get("ImageVersion"),
        "workflow_run": os.environ.get("GITHUB_RUN_ID"),
        "workflow_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "signing": "No signing credential is supplied by this workflow.",
        "acceptance": "Build and automated checks only; interactive desktop acceptance is separate.",
    }
    write_json(output / f"build-{platform}.json", manifest)
    write_checksums(output, f"SHA256SUMS-{platform}.txt")


def validate_manifest(manifest, platform, version, commit, root, installer_dir):
    _, extension = PLATFORMS[platform]
    filename = f"xyzaishell-{version}-{platform}{extension}"
    required = {"schema_version": 1, "platform": platform, "version": version,
                "source_commit": commit, "package_file": filename,
                "bundle": PLATFORMS[platform][0]}
    for key, value in required.items():
        if manifest.get(key) != value:
            raise ValueError(f"{platform}: invalid {key}")
    package = Path(installer_dir) / filename
    if not package.is_file() or package.is_symlink() or sha256(package) != manifest.get("package_sha256"):
        raise ValueError(f"{platform}: installer checksum mismatch")
    for name in ("Cargo.lock", "package-lock.json"):
        if manifest.get("lockfiles", {}).get(name) != sha256(Path(root) / name):
            raise ValueError(f"{platform}: {name} differs from release checkout")
    return package


def release(root, installers, output, tag):
    root, installers, output = Path(root).resolve(), Path(installers).resolve(), Path(output).resolve()
    version = check_version(root, tag)
    commit = command("git", "rev-parse", "HEAD", cwd=root)
    output.mkdir(parents=True, exist_ok=False)
    for platform in PLATFORMS:
        matches = list(installers.rglob(f"build-{platform}.json"))
        if len(matches) != 1:
            raise ValueError(f"Expected exactly one provenance file for {platform}")
        manifest = read_json(matches[0])
        package = validate_manifest(manifest, platform, version, commit, root, matches[0].parent)
        shutil.copyfile(package, output / package.name)
        shutil.copyfile(matches[0], output / matches[0].name)
    prefix = f"xyzaishell-{version}/"
    subprocess.run(["git", "archive", "--format=zip", f"--prefix={prefix}", "-o", str(output / f"xyzaishell-{version}-source.zip"), commit], check=True, cwd=root)
    archive = subprocess.check_output(["git", "archive", "--format=tar", f"--prefix={prefix}", commit], cwd=root)
    (output / f"xyzaishell-{version}-source.tar.gz").write_bytes(gzip.compress(archive, mtime=0))
    write_checksums(output, "SHA256SUMS.txt")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    version = sub.add_parser("check-version")
    version.add_argument("--tag")
    collect_parser = sub.add_parser("collect")
    collect_parser.add_argument("--platform", choices=PLATFORMS, required=True)
    collect_parser.add_argument("--bundle", choices=("deb", "nsis"), required=True)
    collect_parser.add_argument("--output", required=True)
    release_parser = sub.add_parser("release")
    release_parser.add_argument("--tag", required=True)
    release_parser.add_argument("--installers", required=True)
    release_parser.add_argument("--output", required=True)
    args = parser.parse_args()
    if args.action == "check-version":
        print(check_version(Path.cwd(), args.tag))
    elif args.action == "collect":
        collect(Path.cwd(), args.output, args.platform, args.bundle)
    else:
        release(Path.cwd(), args.installers, args.output, args.tag)


if __name__ == "__main__":
    main()
