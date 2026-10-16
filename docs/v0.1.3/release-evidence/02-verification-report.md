# Candidate artifact verification

Run: https://github.com/yanch123/xyzaishell/actions/runs/38840120622
Source: 00660aa73474827cbd78e6f821c3e2b9a254dad2
Verified 2026-10-16 UTC. Run completed successfully, attempt 1; all 11 jobs successful.

## Independently checked
- Downloaded all eight original artifact ZIP archives; their byte sizes and SHA256 digests match GitHub API metadata in final-artifacts.json.
- Both installer manifests identify version 0.1.3, exact source and run 38840120622. Package and manifest SHA256 checksums pass. Both lockfile hashes match git show of the exact source commit.
- Linux DEB: 12,090,414 bytes; SHA256 858255b5afcb98a1d36dea409cd069ee25d42463c85efd6c4f022a9710bae258.
- Windows NSIS EXE: 6,601,373 bytes; SHA256 e8cf55f98421e50840c9438c364920603afc91088af810e82f63de0b714925c6.
- All 15 native result.json files pass: initial install/restart, same-version reinstall, uninstall/reinstall, real v0.1.2 baseline install/restart, direct upgrade to v0.1.3, each on Ubuntu 22.04, Ubuntu 24.04 and Windows Server 2022.
- All three upgrade phase results explicitly report upgrade_from=0.1.2, application_version=0.1.3, history_rows_and_private_v2_profiles_preserved=true and profile_id_preserved=true.
- All phases report actual native IPC, persisted profile/history, preserved task identity, fresh-process history consent, zero restore model requests, and fail-closed empty shell-log export.
- Browser job log shows 14/14 tests passed across wide/compact Chromium. Artifact contains 50 actual screenshot PNGs, all decoded. Native artifacts contain 29 PNGs each (87 total), all decoded. Their pixels were viewed in contact sheets: real rendered application panels, consent, settings, restoration, and expected rejected-export UI; no blank/error-page screenshot substitution observed. This is contact-sheet review, not exhaustive full-resolution visual acceptance.

## Limits preserved
- Unsigned installer builds. These are CI observations, not a local rerun of installed packages.
- Native model responses are synthetic loopback fixtures; browser tests use mocks. Live provider behavior is untested.
- SSH-through-UI, full desktop input methods/IME/clipboard, full X11/Wayland experience, Windows 10/11 and SmartScreen UX are untested. Separate real OpenSSH/current Bash PTY integration jobs passed on Ubuntu 22.04/24.04.
- Legacy profile migration is explicitly false/not tested; the real version-upgrade fixture preserves private-v2 profiles.
- Top-level native result.txt files say version upgrade is not tested in that lifecycle fixture. Separate version-upgrade/result.txt and JSONs do verify the direct real-version upgrade. Do not mistake the top-level fixture limitation for failure of the separate upgrade phase.
- Windows driver provenance reports Microsoft Authenticode Valid and matching 131.0.2903.86 runtime/driver. Its SHA256 records downloaded bytes, not a vendor-published checksum manifest.

Original evidence is retained under this directory with final-run.json, final-jobs.json, final-artifacts.json, native-results-summary.json, browser-job.log, extracted archives and contact sheets. Artifacts expire 2026-10-30 according to API metadata.
