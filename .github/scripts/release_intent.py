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
DESKTOP_EVIDENCE = {"windows_10", "windows_11", "ubuntu_22_04_desktop", "ubuntu_24_04_desktop"}


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
    require(value["mode"] in ("draft", "prerelease", "publish"), "Mode must be draft, prerelease or publish")
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
        if value["mode"] == "prerelease" and category not in DESKTOP_EVIDENCE:
            require(record["status"] != "pending", f"Prerelease blocked by pending {category} acceptance")
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


def verify_activation_tree(root, source, activation, tag):
    require(isinstance(tag, str) and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag), "Stable vX.Y.Z tag required")
    # Only the first release used a minor-version documentation directory.
    evidence_version = "v0.1" if tag == "v0.1.0" else tag
    evidence_root = f"docs/{evidence_version}/release-evidence/"
    for sha in (source, activation):
        require(re.fullmatch(r"[0-9a-f]{40}", sha), "Exact source and activation SHAs required")
    subprocess.run(["git", "merge-base", "--is-ancestor", source, activation], cwd=root, check=True)
    changed = subprocess.check_output(
        ["git", "diff", "--no-renames", "--name-only", "-z", source, activation], cwd=root).decode().split("\0")
    changed = [path for path in changed if path]
    require(".github/release-intent.json" in changed, "Activation must explicitly change the release intent")
    for path in changed:
        require(path == ".github/release-intent.json" or path.startswith(evidence_root),
                f"Untested change between candidate and activation: {path}")
        entry = subprocess.check_output(["git", "ls-tree", activation, "--", path], cwd=root, text=True).strip()
        require(not entry or entry.startswith("100644 blob "), "Release-control/evidence files must be regular non-executable blobs")


def notes(value, run_url):
    require(run_url.startswith("https://github.com/") and "/actions/runs/" in run_url, "Expected GitHub verification run URL")
    label = {"draft": "草稿", "prerelease": "预发布试用版", "publish": "正式发布"}[value["mode"]]
    lines = [f"# xyzaishell {value['tag']} · {label}", "", f"精确源码提交：{value['source_commit']}",
             f"同一源码的自动化验证：{run_url}", "",
             "## 下载与安装", "",
             "下方 Assets 提供 Windows x64 NSIS 安装包（.exe）、Ubuntu x64 安装包（.deb）、源码 ZIP/TAR.GZ、两平台 build manifest 和 SHA256SUMS.txt。请按系统下载安装包，并核对 SHA256。",
             "DEB 在 Ubuntu 22.04 构建，并在 Ubuntu 22.04/24.04 验证。Windows 自动化使用 Windows Server 2022。原生 UI/生命周期测试使用合成数据，不等于 Windows 10/11 或完整用户桌面验收。",
             "安装包未签名，Windows 可能显示未知发布者或 SmartScreen 提示；没有配置代码签名或自动更新签名。",
             "同版本重装不等于跨版本升级。第三方告知、Rust 标准库告知及适用的对应源码已包含在安装包内。",
             f"使用说明：https://github.com/yanch123/xyzaishell/blob/{value['source_commit']}/README.md", "",
             "## 测试状态与限制", "",
             "真实模型供应商、安装后 UI 连接外部 SSH、完整输入法/剪贴板/Wayland 体验和长期压力测试仍不在当前自动化证明范围内。",
             "以下是发布负责人明确记录的验收状态；自动化校验记录并重跑 CI，不能独立证明人工桌面测试已发生。", ""]
    for category in sorted(REQUIRED_EVIDENCE):
        record = value["acceptance"][category]
        lines += [f"- {category}: {record['status']}", f"  {record['notes']}"]
        lines += [f"  {link}" for link in record["evidence"]]
    if value["mode"] == "draft":
        lines += ["", "草稿：尚未公开发布。正式发布前必须完成全部必需验收。"]
    elif value["mode"] == "prerelease":
        lines += ["", "预发布试用版：保留 pending 的桌面验收尚未完成，不作为正式稳定版或 Latest。请先在非生产环境试用；不会把 pending 自动改为 passed。"]
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
        verify_activation_tree(Path.cwd(), value["source_commit"], args.activation_sha, value["tag"])
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
