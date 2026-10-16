#!/usr/bin/env bash
# Linux-only real OpenSSH/OS PTY fixture. No real credentials or known_hosts used.
#   Local/cloud, without account or global config changes:
#     bash .github/scripts/openssh-test.sh --current-user
#   Disposable Ubuntu CI runner (requires CI=true and passwordless sudo):
#     sudo apt-get install --no-install-recommends -y openssh-server
#     bash .github/scripts/openssh-test.sh --disposable-user
# Dependencies: sshd, ssh-keygen, Python 3, setsid, Rust/Cargo. Installation is
# deliberately outside this script. Never run --disposable-user on a real host.
# Unprivileged mode also needs usable /dev/pts and an unlocked current account;
# Integration acceptance exercises implemented Bash adapters; other shells
# remain manual-only in the application. Disposable CI creates a Bash account.
# Restricted containers may authenticate but refuse OS PTY allocation. That is
# a failure, not a skip. Its shell uses the current account's normal login setup.
set -euo pipefail
umask 077

mode=${1:---current-user}
if [[ $# -gt 1 || ! "$mode" =~ ^--(current-user|disposable-user)$ ]]; then
  echo 'Usage: openssh-test.sh [--current-user|--disposable-user]' >&2
  exit 2
fi
[[ $(uname -s) == Linux ]] || { echo 'This fixture requires Linux.' >&2; exit 1; }
sshd=$(command -v sshd || true)
[[ -n "$sshd" ]] || sshd=/usr/sbin/sshd
[[ -x "$sshd" ]] || { echo 'Missing sshd; install openssh-server first.' >&2; exit 1; }
for command in ssh-keygen python3 setsid cargo; do
  command -v "$command" >/dev/null || { echo "Missing prerequisite: $command" >&2; exit 1; }
done
as_root=()
if [[ "$mode" == --disposable-user ]]; then
  [[ ${CI:-} == true ]] || { echo '--disposable-user requires CI=true on a disposable runner.' >&2; exit 1; }
  for command in useradd userdel install getent pkill pgrep runuser cmp locale; do
    command -v "$command" >/dev/null || { echo "Missing prerequisite: $command" >&2; exit 1; }
  done
  if [[ $EUID -ne 0 ]]; then
    command -v sudo >/dev/null || { echo 'Disposable account creation requires sudo.' >&2; exit 1; }
    sudo -n true
    as_root=(sudo -n)
  fi
else
  [[ $EUID -ne 0 ]] || { echo 'Refusing to open a root shell; use a non-root account.' >&2; exit 1; }
fi

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
fixture=$(mktemp -d /tmp/xyz-openssh.XXXXXXXX)
daemon_pid=''
launcher_pid=''
fixture_user=''
fixture_uid=''
created_sshd_runtime=false

cleanup() {
  local result=$? failed=0
  trap - EXIT INT TERM
  if [[ -n "$daemon_pid" ]]; then
    "${as_root[@]}" kill -TERM -- "-$daemon_pid" 2>/dev/null || true
    "${as_root[@]}" kill -TERM "$daemon_pid" 2>/dev/null || true
  elif [[ -n "$launcher_pid" ]]; then
    "${as_root[@]}" kill -TERM "$launcher_pid" 2>/dev/null || true
  fi
  if [[ -n "$launcher_pid" ]]; then
    wait "$launcher_pid" 2>/dev/null || true
  fi
  if [[ -n "$daemon_pid" ]] && "${as_root[@]}" kill -0 "$daemon_pid" 2>/dev/null; then
    echo 'Fixture daemon did not stop.' >&2
    failed=1
  fi
  if [[ -n "$fixture_user" ]]; then
    # This UID was created by this run, never an existing user or the runner.
    "${as_root[@]}" pkill -TERM -u "$fixture_uid" 2>/dev/null || true
    for _ in $(seq 1 30); do
      pgrep -u "$fixture_uid" >/dev/null || break
      sleep 0.1
    done
    "${as_root[@]}" pkill -KILL -u "$fixture_uid" 2>/dev/null || true
    "${as_root[@]}" userdel "$fixture_user" || failed=1
    if getent passwd "$fixture_user" >/dev/null; then failed=1; fi
  fi
  if [[ $result -ne 0 && -f "$fixture/sshd.log" ]]; then
    echo 'OpenSSH fixture diagnostics (synthetic account/public key metadata only):' >&2
    cat "$fixture/sshd.log" >&2 || failed=1
  fi
  "${as_root[@]}" rm -rf -- "$fixture" || failed=1
  [[ ! -e "$fixture" ]] || failed=1
  if [[ "$created_sshd_runtime" == true ]]; then
    "${as_root[@]}" rmdir /run/sshd || failed=1
  fi
  if [[ $failed -ne 0 ]]; then
    echo 'Fixture cleanup failed; inspect the disposable runner before reuse.' >&2
    [[ $result -ne 0 ]] || result=1
  elif [[ $result -eq 0 ]]; then
    echo 'PASS: daemon stopped; temporary credentials and fixture removed.'
  fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

mkdir "$fixture/client" "$fixture/server"
ssh-keygen -q -t ed25519 -N '' -C 'xyzaishell-ephemeral-client' -f "$fixture/client/key"
ssh-keygen -q -t ed25519 -N '' -C 'xyzaishell-ephemeral-host' -f "$fixture/server/host_key"
username=$(id -un)
fixture_bash=${XYZ_TEST_BASH:-/bin/bash}
[[ -x "$fixture_bash" && "$fixture_bash" == /*/bash ]] || { echo 'XYZ_TEST_BASH must be an executable absolute path ending /bash.' >&2; exit 1; }
if [[ "$mode" == --current-user ]]; then
  login_shell=$(getent passwd "$username" | cut -d: -f7)
  [[ $(readlink -f "$login_shell") == $(readlink -f "$fixture_bash") ]] || {
    echo 'Current-user acceptance requires the selected Bash to match the current account shell; no login-shell setting will be changed.' >&2
    exit 1
  }
fi
"$fixture_bash" --noprofile --norc -c '[[ ( ${BASH_VERSINFO[0]} == 4 && ${BASH_VERSINFO[1]} == 2 ) || ( ${BASH_VERSINFO[0]} == 5 && ( ${BASH_VERSINFO[1]} == 1 || ${BASH_VERSINFO[1]} == 2 ) ) ]]' || {
  echo 'Selected runtime has no tested Bash adapter; unsupported shells are not a passing integration test.' >&2
  exit 1
}
fixture_bash_version=$("$fixture_bash" --noprofile --norc -c 'printf "%s" "$BASH_VERSION"')
if [[ -n ${XYZ_TEST_BASH_VERSION:-} && "$fixture_bash_version" != "$XYZ_TEST_BASH_VERSION("* ]]; then
  echo "Expected Bash $XYZ_TEST_BASH_VERSION, got $fixture_bash_version" >&2
  exit 1
fi
printf 'OpenSSH fixture selected real Bash %s (%s)\n' "$fixture_bash_version" "$fixture_bash"
authorized_keys="$fixture/client/authorized_keys"
cp "$fixture/client/key.pub" "$authorized_keys"
if [[ "$mode" == --disposable-user ]]; then
  # A custom runtime may live beneath runner-private ancestors. Copy only the
  # selected executable into this disposable fixture, never widen runner-home
  # permissions. The exact bytes and runtime version remain independently checked.
  "${as_root[@]}" install -d -m 755 "$fixture/runtime"
  "${as_root[@]}" install -m 755 "$fixture_bash" "$fixture/runtime/bash"
  cmp -- "$fixture_bash" "$fixture/runtime/bash" || {
    echo 'Disposable runtime copy differs from selected Bash.' >&2
    exit 1
  }
  fixture_bash="$fixture/runtime/bash"
  username="xyzssh_$(python3 -c 'import secrets; print(secrets.token_hex(6))')"
  # '*' has no usable password but is not OpenSSH's '!' locked-account prefix.
  # Password auth is also disabled in this daemon's isolated configuration.
  # --system already suppresses mail-spool creation. CREATE_MAIL_SPOOL belongs
  # to /etc/default/useradd and is not a valid --key login.defs override.
  "${as_root[@]}" useradd --system --no-log-init --no-create-home --no-user-group \
    --home-dir "$fixture/home" \
    --shell "$fixture_bash" --password '*' "$username"
  fixture_user=$username
  fixture_uid=$(id -u "$username")
  "${as_root[@]}" install -d -m 700 -o "$fixture_uid" "$fixture/home" "$fixture/home/.ssh"
  # Configure only the account created by this fixture, before SSH startup.
  # Production preparation must preserve these settings and definitions.
  cat > "$fixture/bashrc" <<'BASHRC'
# sshd does not inherit the runner's locale. In the C locale, native Bash 4.2
# readline interprets UTF-8 input bytes as meta keys instead of inserting them.
# The Unicode-input acceptance case requires a verified UTF-8 fixture locale.
export LC_ALL=C.UTF-8
stty -echo
HISTCONTROL=ignoreboth
HISTIGNORE=
HISTSIZE=1000
shopt -s cmdhist promptvars lithist
PS1='XYZ_OPENSSH> '
alias xyz_fixture_alias='printf "%s\n" XYZ_ALIAS_PRESERVED'
xyz_fixture_function() { return 23; }
BASHRC
  "${as_root[@]}" install -m 600 -o "$fixture_uid" "$fixture/bashrc" "$fixture/home/.bashrc"
  export XYZ_OPENSSH_FIXTURE_BASHRC=1
  authorized_keys="$fixture/home/.ssh/authorized_keys"
  "${as_root[@]}" install -m 600 -o "$fixture_uid" "$fixture/client/key.pub" "$authorized_keys"
  chmod 711 "$fixture"
  # The daemon executes as this account, not the CI runner. Verify all parent
  # directories and runtime dependencies are usable before starting SSH.
  account_version=$("${as_root[@]}" runuser -u "$username" -- "$fixture_bash" \
    --noprofile --norc -c 'printf "%s" "$BASH_VERSION"') || {
    echo 'Disposable account cannot execute selected Bash runtime.' >&2
    exit 1
  }
  [[ "$account_version" == "$fixture_bash_version" ]] || {
    echo "Disposable account runtime mismatch: $account_version" >&2
    exit 1
  }
  account_charmap=$("${as_root[@]}" runuser -u "$username" -- "$fixture_bash" \
    --noprofile --norc -c 'export LC_ALL=C.UTF-8; locale charmap') || {
    echo 'Disposable account cannot use the fixture UTF-8 locale.' >&2
    exit 1
  }
  [[ "$account_charmap" == UTF-8 ]] || {
    echo "Disposable account needs UTF-8 for Unicode input, got $account_charmap" >&2
    exit 1
  }
  # Some minimal images omit this package-owned privilege-separation directory.
  # Only the disposable-account mode may create it, and removes it if created.
  if [[ ! -d /run/sshd ]]; then
    "${as_root[@]}" install -d -m 755 /run/sshd
    created_sshd_runtime=true
  fi
fi

port=$(python3 - <<'PY'
import socket
with socket.socket() as listener:
    listener.bind(('127.0.0.1', 0))
    print(listener.getsockname()[1])
PY
)
cat > "$fixture/sshd_config" <<CONFIG
AddressFamily inet
ListenAddress 127.0.0.1
Port $port
HostKey $fixture/server/host_key
HostKeyAlgorithms ssh-ed25519
PidFile $fixture/sshd.pid
AuthorizedKeysFile $authorized_keys
StrictModes yes
AllowUsers $username
AuthenticationMethods publickey
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
PermitRootLogin no
UsePAM no
PermitTTY yes
PermitUserRC no
PermitUserEnvironment no
DisableForwarding yes
X11Forwarding no
MaxSessions 1
LoginGraceTime 30
PrintMotd no
PrintLastLog no
LogLevel DEBUG3
CONFIG

"${as_root[@]}" "$sshd" -t -f "$fixture/sshd_config"
"${as_root[@]}" setsid --wait "$sshd" -D -e -f "$fixture/sshd_config" > "$fixture/sshd.log" 2>&1 &
launcher_pid=$!
for _ in $(seq 1 100); do
  # The sudo/setsid launcher may already run as root. An unprivileged kill -0
  # can return EPERM for a live process; probe with the launcher's privileges.
  if ! "${as_root[@]}" kill -0 "$launcher_pid" 2> "$fixture/launcher-status.log"; then
    # Do not turn an unexpected probe-permission failure into an unbounded wait
    # on a still-running child. Bash tracks this launcher's job without signals.
    if jobs -pr | grep -qx "$launcher_pid"; then
      echo 'Cannot inspect the still-running sshd launcher; refusing ambiguous startup.' >&2
      cat "$fixture/launcher-status.log" >&2
      exit 1
    fi
    if wait "$launcher_pid"; then launcher_status=0; else launcher_status=$?; fi
    echo "sshd launcher exited during startup (exit status $launcher_status)." >&2
    cat "$fixture/launcher-status.log" >&2
    exit 1
  fi
  if grep -q "Server listening on 127.0.0.1 port $port" "$fixture/sshd.log" \
    && [[ -f "$fixture/sshd.pid" ]]; then
    daemon_pid=$("${as_root[@]}" cat "$fixture/sshd.pid")
    [[ "$daemon_pid" =~ ^[0-9]+$ ]] || { echo 'Invalid fixture PID.' >&2; exit 1; }
    break
  fi
  sleep 0.1
done
[[ -n "$daemon_pid" ]] || { echo 'sshd never became ready.' >&2; exit 1; }

export XYZ_OPENSSH_PORT=$port XYZ_OPENSSH_USER=$username
export XYZ_OPENSSH_BASH_VERSION=$fixture_bash_version
export XYZ_OPENSSH_PRIVATE_KEY="$fixture/client/key"
export XYZ_OPENSSH_HOST_FINGERPRINT
XYZ_OPENSSH_HOST_FINGERPRINT=$(ssh-keygen -lf "$fixture/server/host_key.pub" -E sha256 | awk '{print $2}')
# Version output is diagnostic only: OpenSSH 8.9 prints its version/usage
# but exits nonzero for -V, which newer releases support. The daemon above
# already passed configuration validation and the readiness check.
"$sshd" -V 2>&1 || true
cd "$repo"
# --exact plus --ignored prevents silently passing without executing acceptance.
cargo test -p xyz-core --locked --test openssh_interop \
  openssh_real_pty_shell_unicode_resize_and_disconnect -- --ignored --exact --nocapture
cargo test -p xyz-core --locked --test shell_log_openssh \
  shell_log_real_openssh_records_commands_without_model -- --ignored --exact --nocapture

# OpenSSH records protocol request types, independently of the client's events.
# Two terminal-interoperability plus three shell-log connections must each
# allocate exactly one PTY and start its controlled shell with one exec.
# There is no hidden channel or stdin bootstrap; subsystem requests are forbidden.
python3 - "$fixture/sshd.log" <<'PY'
import pathlib
import re
import sys

log = pathlib.Path(sys.argv[1]).read_text()
for request, count in [('pty-req', 5), ('exec', 5), ('shell', 0), ('window-change', 1)]:
    actual = len(re.findall(r'server_input_channel_req: channel \d+ request ' + request + r' reply ', log))
    assert actual == count, f'Expected {count} {request} requests, got {actual}'
assert not re.search(r'request subsystem reply ', log), 'Unexpected subsystem request detected'
assert len(re.findall(r'server_input_channel_open: ctype session ', log)) == 5, 'Unexpected extra session channel'
assert len(re.findall(r'Starting session: command on ', log)) == 5, 'Expected five controlled real OS PTY shells'
print('PASS: OpenSSH observed five controlled PTY exec shells, resize, and no extra channels or subsystem requests.')
PY
