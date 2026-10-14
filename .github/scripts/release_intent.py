#!/usr/bin/env python3
"""Validate a deliberate main-branch release request; never publish by itself."""
import argparse
import json
import re
import subprocess
from pathlib import Path
from urllib.parse import urlsplit

REQUIRED_EVIDENCE = {
    "windows_10", "windows_11", "ubuntu_22_04_desktop", "ubuntu_24_04_desktop",
    "installation_lifecycle", "ssh_model_security", "dependency_notices", "version_upgrade",
}


def require(value, message):
    if not value:
        raise ValueError(message)


def validate(value):
    require(isinstance(value, dict), "Release intent must be an object")
    require(set(value) == {"schema_version", "tag", "source_commit", "mode", "confirmation", "acceptance"},
            "Unknown or missing release-intent field")
    require(type(value["schema_version"]) is int and value["schema_version"] == 1, "Unsupported intent schema")
    require(isinstance(value["tag"], str) and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", value["tag"]), "Stable vX.Y.Z tag required")
    require(isinstance(value["source_commit"], str) and re.fullmatch(r"[0-9a-f]{40}", value["source_commit"]), "Exact lowercase commit SHA required")
    require(value["mode"] in ("draft", "publish"), "Mode must be draft or publish")
    expected = f"{value['mode']} xyzaishell {value['tag']} from {value['source_commit']}"
    require(value["confirmation"] == expected, "Exact version/source/mode confirmation required")
    records = value["acceptance"]
    require(isinstance(records, dict) and set(records) == REQUIRED_EVIDENCE, "All acceptance categories must be explicit")
    for category, record in records.items():
        require(isinstance(record, dict) and set(record) == {"status", "evidence", "notes"}, "Invalid acceptance record")
        require(record["status"] in ("passed", "pending", "not_applicable"), "Invalid acceptance status")
        require(isinstance(record["notes"], str) and 1 <= len(record["notes"]) <= 2000
                and not any(ord(c) < 32 and c != "\n" for c in record["notes"]), "Bounded nonempty acceptance notes required")
        require(isinstance(record["evidence"], list) and len(record["evidence"]) <= 10, "Evidence must be a bounded URL list")
        for link in record["evidence"]:
            require(isinstance(link, str) and len(link) <= 2000 and not any(c.isspace() for c in link), "Invalid evidence URL")
            url = urlsplit(link)
            require(url.scheme == "https" and url.hostname and not url.username and not url.password,
                    "Evidence URLs require HTTPS and no credentials")
        if record["status"] == "passed":
            require(record["evidence"], "Passed acceptance requires evidence links")
        if record["status"] == "not_applicable":
            require(category == "version_upgrade" and value["tag"] == "v0.1.0",
                    "Only first-release version upgrade may be not applicable")
        if value["mode"] == "publish":
            require(record["status"] != "pending", f"Publication blocked by pending {category} acceptance")
    return value


def read(path):
    path = Path(path)
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 65536, "Intent must be a small regular file")
    # Duplicate keys must not let reviewers and different parsers see different intent.
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "Duplicate JSON key")
            result[key] = value
        return result
    return validate(json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique))


def verify_activation_tree(root, source, activation):
    for sha in (source, activation):
        require(re.fullmatch(r"[0-9a-f]{40}", sha), "Exact source and activation SHAs required")
    subprocess.run(["git", "merge-base", "--is-ancestor", source, activation], cwd=root, check=True)
    changed = subprocess.check_output(
        ["git", "diff", "--no-renames", "--name-only", "-z", source, activation], cwd=root).decode().split("\0")
    changed = [path for path in changed if path]
    require(".github/release-intent.json" in changed, "Activation must explicitly change the release intent")
    for path in changed:
        require(path == ".github/release-intent.json" or path.startswith("docs/v0.1/release-evidence/"),
                f"Untested change between candidate and activation: {path}")
        entry = subprocess.check_output(["git", "ls-tree", activation, "--", path], cwd=root, text=True).strip()
        require(not entry or entry.startswith("100644 blob "), "Release-control/evidence files must be regular non-executable blobs")


def notes(value, run_url):
    require(run_url.startswith("https://github.com/") and "/actions/runs/" in run_url, "Expected GitHub verification run URL")
    lines = [f"# xyzaishell {value['tag']}", "", f"Source commit: {value['source_commit']}",
             f"Exact-source automated verification: {run_url}", "",
             "Includes source ZIP/TAR.GZ, Windows x64 NSIS, Ubuntu x64 DEB, build provenance and SHA256SUMS.txt.",
             "The DEB is built on Ubuntu 22.04. Automated native UI/lifecycle tests run on Ubuntu 22.04/24.04 and Windows Server 2022 using synthetic data. They do not establish Windows 10/11 or full desktop acceptance by themselves.",
             "Installers are unsigned; Windows may show unknown-publisher or SmartScreen warnings. No updater-signing key or code-signing identity is configured.",
             "Same-version reinstall is not a version-upgrade test. Third-party notices and relevant source materials are included inside the installers.",
             "", "## Release-owner acceptance records", "",
             "These records are explicit release-owner attestations. Automation validates their structure and reruns CI; it cannot independently certify a manual desktop test.", ""]
    for category in sorted(REQUIRED_EVIDENCE):
        record = value["acceptance"][category]
        lines += [f"- {category}: {record['status']}", f"  {record['notes']}"]
        lines += [f"  {link}" for link in record["evidence"]]
    if value["mode"] == "draft":
        lines += ["", "DRAFT: pending acceptance must be completed before any explicit publication."]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("path", type=Path)
    parser.add_argument("--canonical-output", type=Path)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--notes-output", type=Path)
    parser.add_argument("--run-url")
    parser.add_argument("--activation-sha")
    args = parser.parse_args()
    value = read(args.path)
    if args.activation_sha:
        verify_activation_tree(Path.cwd(), value["source_commit"], args.activation_sha)
    if args.canonical_output:
        args.canonical_output.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            output.write(f"tag={value['tag']}\nsha={value['source_commit']}\nmode={value['mode']}\n")
    if args.notes_output:
        args.notes_output.write_text(notes(value, args.run_url or ""), encoding="utf-8")
    print(f"Validated {value['mode']} intent for {value['tag']} at {value['source_commit']}")


if __name__ == "__main__":
    main()
