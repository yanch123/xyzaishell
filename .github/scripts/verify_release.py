#!/usr/bin/env python3
"""Read-only validation of fetched GitHub release state and all uploaded bytes."""
import argparse
import hashlib
import json
from pathlib import Path

from release_intent import read, require


def asset_names(tag):
    version = tag[1:]
    return {f"xyzaishell-{version}-{suffix}" for suffix in (
        "linux-x64.deb", "windows-x64.exe", "source.zip", "source.tar.gz"
    )} | {"build-linux-x64.json", "build-windows-x64.json", "SHA256SUMS.txt"}


def verify(intent, release, summary, tag_ref, directory, draft_stage=False):
    mode = "draft" if draft_stage else intent["mode"]
    tag = intent["tag"]
    expected = {"isDraft": mode == "draft", "isPrerelease": mode == "prerelease", "isLatest": mode == "publish"}
    require(isinstance(release, dict) and release.get("tag_name") == tag, "Release tag mismatch")
    require(type(release.get("id")) is int and release["id"] > 0, "Missing release ID")
    require(release.get("draft") is expected["isDraft"] and release.get("prerelease") is expected["isPrerelease"],
            "Remote release draft/prerelease state mismatch")
    require(isinstance(summary, list) and len(summary) == 1 and summary[0].get("tagName") == tag,
            "Expected exactly one release summary for the tag")
    require(all(summary[0].get(key) is value for key, value in expected.items()), "Remote draft/prerelease/latest state mismatch")
    require(tag_ref.get("ref") == f"refs/tags/{tag}" and tag_ref.get("object", {}).get("type") == "commit"
            and tag_ref["object"].get("sha") == intent["source_commit"], "Tag no longer points directly to the exact candidate")
    directory = Path(directory)
    names = asset_names(tag)
    local = list(directory.iterdir())
    require({p.name for p in local} == names and all(p.is_file() and not p.is_symlink() and p.stat().st_size > 0 for p in local),
            "Expected exactly seven nonempty regular release assets")
    assets = release.get("assets")
    require(isinstance(assets, list) and len(assets) == len(names)
            and {a.get("name") for a in assets} == names, "Missing, duplicate or unexpected remote release assets")
    for asset in assets:
        path = directory / asset["name"]
        require(asset.get("state") == "uploaded" and type(asset.get("size")) is int
                and asset["size"] == path.stat().st_size, f"Incomplete uploaded asset: {path.name}")
        digest = "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
        require(asset.get("digest") == digest, f"Remote asset digest mismatch: {path.name}")
    return mode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--intent", type=Path, required=True)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    parser.add_argument("--tag-ref", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--draft-stage", action="store_true")
    args = parser.parse_args()
    def load(path):
        return json.loads(path.read_text(encoding="utf-8"))
    intent = read(args.intent)
    mode = verify(intent, load(args.release), load(args.summary), load(args.tag_ref), args.assets, args.draft_stage)
    print(f"Verified {mode}: exact {intent['tag']} / {intent['source_commit']}, all 7 uploaded asset sizes and SHA256 digests, and draft/prerelease/latest flags.")


if __name__ == "__main__":
    main()
