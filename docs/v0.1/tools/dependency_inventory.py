#!/usr/bin/env python3
"""Capture local dependency license evidence. This is NOT a compliance approval.

Requires Python 3.10+, npm ci, and cargo fetch --locked. Output must not exist.
No network, dependency installation, SPDX-choice selection, or publication occurs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess



def sha(data):
    return hashlib.sha256(data).hexdigest()


def capture(root, destination, explicit=None):
    candidates = set()
    for path in root.rglob('*'):
        relative = path.relative_to(root)
        if 'node_modules' in relative.parts or '.git' in relative.parts:
            continue
        if path.is_file() and re.match(r'^(licen[cs]e|notice|copying|copyright)', path.name, re.I):
            candidates.add(path)
    if explicit:
        candidates.add(root / explicit)
    result = []
    for path in sorted(candidates):
        # Do not follow package symlinks or permit a declared license outside package.
        relative = path.relative_to(root)
        if '..' in relative.parts or path.is_symlink() or not path.is_file():
            continue
        if root.resolve() not in path.resolve().parents:
            continue
        data = path.read_bytes()
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        result.append({'path': relative.as_posix(), 'sha256': sha(data)})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[3])
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--target', action='append', help='Repeat for each release target; defaults to Linux/Windows x64.')
    args = parser.parse_args()
    repo, output = args.repo.resolve(), args.output.resolve()
    if output.exists():
        parser.error('Output must be a new directory; previous evidence is never overwritten.')
    # Metadata is collected internally, so caller cannot accidentally supply stale metadata.
    locks = {name: (repo / name).read_bytes() for name in ('Cargo.lock', 'package-lock.json')}
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--format-version', '1', '--locked', '--offline'], cwd=repo))
    normal_targets = {}
    for target in args.target or ['x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc']:
        tree = subprocess.check_output(
            ['cargo', 'tree', '--locked', '--offline', '-p', 'xyzaishell', '--target', target,
             '--edges', 'normal,no-proc-macro', '--prefix', 'none', '--format', '{p}'],
            cwd=repo, text=True)
        pairs = set()
        for line in tree.splitlines():
            match = re.match(r'(\S+) v(\S+)', line)
            if match:
                pairs.add(match.groups())
        for package in metadata['packages']:
            if (package['name'], package['version']) in pairs:
                normal_targets.setdefault(package['id'], []).append(target)
    # Cargo-generated lock entries use plain quoted values for these fields.
    # Avoid a TOML dependency on Python 3.10 (Ubuntu 22.04).
    checksum = {}
    for block in locks['Cargo.lock'].decode().split('[[package]]')[1:]:
        fields = {key: json.loads(value) for key, value in re.findall(
            r'^(name|version|source|checksum) = ("[^"\n]*")$', block, re.M)}
        checksum[(fields['name'], fields['version'], fields.get('source'))] = fields.get('checksum')
    output.mkdir(parents=True)
    records = []
    for package in sorted(metadata['packages'], key=lambda p: p['id']):
        if not package.get('source'):
            continue
        name, version, source = package['name'], package['version'], package['source']
        identity = (name, version, source)
        if identity not in checksum:
            raise RuntimeError(f'Metadata package absent from lock: {identity}')
        key = f'cargo/{name}-{version}-{sha(source.encode())[:12]}'
        package_root = Path(package['manifest_path']).parent
        files = capture(package_root, output / 'texts' / key,
                        package.get('license_file'))
        archive = package_root.parents[2] / 'cache' / package_root.parent.name / f'{name}-{version}.crate'
        source_archive = None
        source_url = None
        if source == 'registry+https://github.com/rust-lang/crates.io-index':
            source_url = f'https://static.crates.io/crates/{name}/{name}-{version}.crate'
        if archive.is_file():
            data = archive.read_bytes()
            if sha(data) != checksum[identity]:
                raise RuntimeError(f'Registry archive checksum mismatch: {identity}')
            if 'MPL-2.0' in (package.get('license') or '') and package['id'] in normal_targets:
                target = output / 'sources' / f'{name}-{version}.crate'
                target.parent.mkdir(exist_ok=True)
                target.write_bytes(data)
                source_archive = {'path': target.relative_to(output).as_posix(), 'sha256': sha(data)}
        vcs_file = package_root / '.cargo_vcs_info.json'
        vcs = json.loads(vcs_file.read_text(encoding='utf-8')) if vcs_file.is_file() else None
        records.append({'ecosystem': 'cargo', 'name': name, 'version': version,
                        'source': source, 'source_url': source_url, 'source_archive': source_archive,
                        'upstream_vcs': vcs, 'lock_checksum': checksum[identity],
                        'license_declared': package.get('license'), 'repository': package.get('repository'),
                        'normal_dependency_targets': normal_targets.get(package['id'], []),
                        'scope': 'normal dependency graph candidate' if package['id'] in normal_targets else 'build/dev/other-target graph only',
                        'evidence_directory': f'texts/{key}', 'files': files,
                        'needs_review': not files or not package.get('license')})
    npm_lock = json.loads(locks['package-lock.json'])
    for location, package in sorted(npm_lock['packages'].items()):
        if not location or package.get('dev'):
            continue
        if not location.startswith('node_modules/') or '..' in Path(location).parts:
            raise RuntimeError(f'Unsupported npm location: {location}')
        root = repo / location
        manifest = root / 'package.json'
        installed = json.loads(manifest.read_text(encoding='utf-8')) if manifest.is_file() else {}
        if installed and installed.get('version') != package['version']:
            raise RuntimeError(f'Installed npm version differs from lock: {location}')
        name = installed.get('name') or location.rsplit('node_modules/', 1)[-1]
        key = f'npm/{location}'
        files = capture(root, output / 'texts' / key) if root.is_dir() else []
        records.append({'ecosystem': 'npm', 'name': name, 'version': package['version'],
                        'source': package.get('resolved'), 'lock_integrity': package.get('integrity'),
                        'license_declared': installed.get('license', package.get('license')),
                        'scope': 'non-dev lock entries; actual frontend inclusion still needs review',
                        'evidence_directory': f'texts/{key}', 'files': files,
                        'needs_review': not files or not installed})
    for name, data in locks.items():
        if (repo / name).read_bytes() != data:
            raise RuntimeError('Lockfile changed during inventory; discard output and retry.')
        (output / name).write_bytes(data)
    report = {'schema_version': 1, 'status': 'evidence-only-not-license-clearance',
              'lock_sha256': {name: sha(data) for name, data in locks.items()},
              'limitations': ['Cargo tree normal,no-proc-macro excludes build/dev/proc-macro paths; linker dead-code elimination is not modeled.',
                              'Does not collect arbitrary source headers, Rust sysroot, OS libraries, or installer/runtime licenses.',
                              'License-file discovery is heuristic; no SPDX expressions are simplified.',
                              'No selected-license decision or source redistribution offer is generated.'],
              'packages': records}
    (output / 'inventory.json').write_text(json.dumps(report, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    print(f'{len(records)} dependency records; {sum(p["needs_review"] for p in records)} missing-evidence flags.')
    print(f'Evidence saved in {output}. Human review is required before distribution.')


if __name__ == '__main__':
    main()
