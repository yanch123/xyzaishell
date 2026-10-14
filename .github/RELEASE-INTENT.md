# Explicit release-intent procedure

This is inactive documentation, not a release request. Do not create
`.github/release-intent.json` until the release owner has approved the exact
version, source commit, publication mode and acceptance evidence.

## Sequence

1. Finish application, workflow, packaging and notice changes on `dev`. Obtain a
   successful complete CI run for the exact candidate commit, including the
   installed native acceptance jobs. Review the screenshots and retained logs.
2. Complete the required human/platform acceptance records honestly. Hosted
   Windows Server tests do not prove Windows 10/11 support. Ubuntu Xvfb tests do
   not prove end-user desktop, IME, clipboard or Wayland behavior. If those tests
   are unavailable, keep them pending. An owner-approved `prerelease` may expose
   those four desktop gaps honestly; `publish` still requires every required
   acceptance to be complete. This choice must be reviewed on `dev`.
3. Add the intent on `dev` with the candidate's complete 40-character SHA. Only
   `.github/release-intent.json` and regular non-executable evidence files inside
   `docs/v0.1/release-evidence/` may differ from the candidate. An application,
   workflow or dependency change requires a newly tested candidate instead.
4. Review the intent and merge/fast-forward the authorized `dev` state to `main`.
   A main push changing the intent triggers the workflow. It confirms ancestry,
   the narrow source boundary, version consistency and absence of an existing
   tag, then reruns all CI for the candidate.
5. Only after verification succeeds, the write-scoped final job creates a fresh
   tag pointing to the candidate and uploads exact-source ZIP/TAR.GZ, NSIS/DEB,
   provenance and checksums as a fresh draft. Before any publication, it reads back
   the tag and all seven assets, checking names, upload state, sizes and SHA256
   digests. The explicit validated mode controls the next step:
   - `draft`: remain private, not a prerelease and not Latest.
   - `prerelease`: publish with `isDraft=false`, `isPrerelease=true`, `isLatest=false`.
     Only the four desktop categories may remain pending. Installation lifecycle,
     SSH/model/security and dependency notices must have passed with evidence;
     first-release version upgrade may be not applicable.
   - `publish`: publish as stable and Latest only after all required acceptance
     is complete; no pending category is allowed.
   The workflow reads the final flags, original release ID, exact tag target and
   all seven asset digests again. A token-created tag need not trigger another
   workflow: this workflow directly invokes verification before creating it.

The final tag points to the tested candidate, not the later control-only intent
commit. Release notes retain the source SHA, verification run and all acceptance
records, with Chinese-first download instructions and explicit limitations.
The main branch still contains the reviewed intent/evidence change.

`v0.1.0` remains the application/tag version for a first prerelease; the GitHub
prerelease flag and notes express its trial status. It is not marked Latest.
This does not silently promote it to stable after later tests. Existing tags
and releases are never replaced or updated by a new intent. A later promotion
requires separately reviewed and explicitly authorized handling.

Required uploaded files are the NSIS `.exe`, `.deb`, source `.zip` and `.tar.gz`,
`build-linux-x64.json`, `build-windows-x64.json`, and `SHA256SUMS.txt`. Built-in
GitHub source links do not substitute for the explicit, checksummed source assets.
The download digests use the [GitHub release asset API](https://docs.github.com/en/rest/releases/assets);
visibility and Latest state are read using the [GitHub CLI](https://cli.github.com/manual/gh_release_list).

## Record schema

The object has exactly these fields:

- `schema_version`: integer `1`
- `tag`: stable `vX.Y.Z`, matching Cargo, Tauri and npm versions
- `source_commit`: exact lowercase 40-character candidate SHA
- `mode`: `draft`, `prerelease` or `publish`
- `confirmation`: exactly `MODE xyzaishell TAG from SHA`, with the actual values
- `acceptance`: exactly the eight categories below

Each acceptance category has exactly `status`, `evidence` and `notes`:

- `windows_10`
- `windows_11`
- `ubuntu_22_04_desktop`
- `ubuntu_24_04_desktop`
- `installation_lifecycle`
- `ssh_model_security`
- `dependency_notices`
- `version_upgrade`

`status` is `passed`, `pending` or `not_applicable`. Passed records require at
least one HTTPS evidence link. Notes must state the actual OS/build, cases,
outcome and limits as appropriate. Never include credentials or personal data.
`not_applicable` is allowed only for `version_upgrade` on first release `v0.1.0`;
same-version reinstall does not establish cross-version upgrade. Pending records
are allowed in draft mode; prerelease allows pending only for the four desktop
categories. No category may be pending in stable publish mode. The validator
checks structure and gates; a link
and a passed label alone are not independent proof that a manual test happened.

The document must be a regular JSON file at most 64 KiB, with no duplicate keys.
Validate a prepared record locally with:

```sh
python3 .github/scripts/release_intent.py PATH_TO_REVIEWED_RECORD
```

No example activation file is installed by this repository, to avoid an
accidental main push triggering a release request.

## Failure and recovery

The workflow refuses to overwrite any existing tag or release. If a failure
occurs after tag or draft creation, inspect the tag target, draft visibility,
assets, checksums and failed step before choosing an explicitly approved recovery.
Do not delete/recreate a published tag, infer that an API error means absence, or
retry publication blindly. A draft created through this route remains draft until
a separate deliberate publication action; changing the same tag's intent does
not overwrite it. This conservatism also means partial release recovery is not
automatic.

Only the final job can write repository contents, using the run's ephemeral
`GITHUB_TOKEN`. There is no PAT, long-lived credential, repository-settings change,
automatic branch merge, code-signing certificate or updater key in this flow.
