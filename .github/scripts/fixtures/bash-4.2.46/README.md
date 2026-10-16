# Historical Bash compatibility fixture

`SHA256SUMS` pins the official GNU `bash-4.2.tar.gz` and patches `bash42-001`
through `bash42-046` downloaded from `https://ftp.gnu.org/gnu/bash/`.
The builder verifies every download before extracting, patching, or compiling.

This intentionally old upstream runtime is for disposable compatibility tests
only. It is not a recommended server installation, a security baseline, or a
replacement for the system shell. Distribution builds with the same version
string may carry additional security backports; version alone cannot establish
security or integration capability.

CI builds it into an isolated directory, verifies the actual `BASH_VERSION`,
then runs both the real PTY lifecycle suite and the loopback-only OpenSSH fixture
with a newly created disposable account. Existing Bash 5.1 and 5.2 matrix entries
remain required. No passing skip is allowed for missing tools or failed PTYs.

Local reproduction, with build prerequisites already installed:

```sh
bash .github/scripts/build-bash-4.2.sh /tmp/xyz-bash42-fresh
XYZ_TEST_BASH=/tmp/xyz-bash42-fresh/bin/bash XYZ_TEST_BASH_VERSION=4.2.46 \
  python3 crates/xyz-core/tests/shell_log_pty.py
```

An optional `XYZ_BASH42_SOURCE_CACHE` may point to previously downloaded source
bytes in the manifest's layout. Cached bytes undergo the same checksum checks.
