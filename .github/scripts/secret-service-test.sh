#!/usr/bin/env bash
# Real Secret Service tests, restricted to a fresh private D-Bus/keyring session.
# Prerequisites: gnome-keyring, dbus, Python 3, and the repository Rust toolchain.
# No sudo, login keyring, real credential, or inherited desktop bus is used.
set -euo pipefail
umask 077
[[ $# -eq 0 && $(uname -s) == Linux ]] || { echo 'Usage: secret-service-test.sh (Linux only)' >&2; exit 2; }
for command in cargo python3 dbus-run-session dbus-send gnome-keyring-daemon timeout; do
  command -v "$command" >/dev/null || { echo "Missing prerequisite: $command" >&2; exit 1; }
done
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
fixture=$(mktemp -d /tmp/xyz-keyring.XXXXXXXX)
cleanup() {
  local result=$?
  trap - EXIT INT TERM
  rm -rf -- "$fixture" || result=1
  [[ ! -e "$fixture" ]] || result=1
  if [[ $result -eq 0 ]]; then
    echo 'PASS: disposable D-Bus session exited and synthetic keyring files removed.'
  fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
cd "$repo"
# Build before replacing HOME, so Cargo never needs access to a user's toolchain
# or registry credentials inside the isolated desktop session. Run the resulting
# libtest executable directly with the same exact/ignored filters as cargo test.
cargo test -p xyz-core --locked --lib --no-run --message-format=json > "$fixture/build.jsonl"
test_binary=$(python3 - "$fixture/build.jsonl" <<'PY'
import json
import pathlib
import sys

executables = set()
for line in pathlib.Path(sys.argv[1]).read_text().splitlines():
    message = json.loads(line)
    if (message.get('reason') == 'compiler-artifact'
            and message.get('target', {}).get('name') == 'xyz_core'
            and message.get('profile', {}).get('test')
            and message.get('executable')):
        executables.add(message['executable'])
if len(executables) != 1:
    raise SystemExit('Expected exactly one xyz-core libtest executable')
print(executables.pop())
PY
)
[[ -x "$test_binary" ]] || { echo 'Missing libtest executable' >&2; exit 1; }
mkdir -p "$fixture/home" "$fixture/config" "$fixture/data" "$fixture/cache" "$fixture/runtime"
# env -i is deliberate: no DBUS_*, GNOME_KEYRING_*, SSH_AUTH_SOCK, DISPLAY,
# WAYLAND_DISPLAY, inherited HOME/XDG, or other desktop session configuration.
timeout --kill-after=5s 180s env -i \
  PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 \
  HOME="$fixture/home" XDG_CONFIG_HOME="$fixture/config" \
  XDG_DATA_HOME="$fixture/data" XDG_CACHE_HOME="$fixture/cache" \
  XDG_RUNTIME_DIR="$fixture/runtime" XYZ_TEST_ISOLATED_KEYRING=1 \
  dbus-run-session -- bash -euo pipefail -s -- "$test_binary" "$fixture" <<'SESSION'
umask 077
test_binary=$1
fixture=$2
daemon_pid=''
cleanup_session() {
  local result=$?
  trap - EXIT INT TERM
  if [[ -n "$daemon_pid" ]]; then
    kill -TERM "$daemon_pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$daemon_pid" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$daemon_pid" 2>/dev/null; then
      echo 'Keyring daemon did not stop promptly.' >&2
      kill -KILL "$daemon_pid" 2>/dev/null || true
      result=1
    fi
    wait "$daemon_pid" 2>/dev/null || true
  fi
  if [[ $result -ne 0 ]]; then
    cat "$fixture/keyring.log" >&2 || true
  fi
  exit "$result"
}
trap cleanup_session EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
[[ -n ${DBUS_SESSION_BUS_ADDRESS:-} ]] || { echo 'No isolated session bus' >&2; exit 1; }
mkdir "$XDG_RUNTIME_DIR/keyring"
# --unlock creates a login collection when absent. The only password/secret in
# this fixture is a public, synthetic test value. Never eval daemon output.
printf '%s' 'xyzaishell-synthetic-disposable-login-password' | \
  gnome-keyring-daemon --foreground --unlock --components=secrets \
    --control-directory="$XDG_RUNTIME_DIR/keyring" > "$fixture/keyring.log" 2>&1 &
daemon_pid=$!
ready=false
for _ in $(seq 1 100); do
  kill -0 "$daemon_pid" 2>/dev/null || { echo 'Keyring daemon exited during startup' >&2; exit 1; }
  if dbus-send --session --print-reply --reply-timeout=1000 \
    --dest=org.freedesktop.DBus /org/freedesktop/DBus \
    org.freedesktop.DBus.NameHasOwner string:org.freedesktop.secrets 2>/dev/null \
    | grep -q 'boolean true'; then
    ready=true
    break
  fi
  sleep 0.1
done
[[ "$ready" == true ]] || { echo 'Secret Service never became ready' >&2; exit 1; }
gnome-keyring-daemon --version
for test_name in \
  storage::tests::native_secret_service_isolated_roundtrip \
  storage::tests::native_secret_service_isolated_locked_is_fail_closed; do
  # A misspelled Rust test filter normally returns success with zero tests.
  # Fail explicitly instead; the locked test must remain last in this session.
  "$test_binary" "$test_name" --exact --ignored --list \
    | grep -Fx "$test_name: test"
  timeout --kill-after=5s 60s "$test_binary" "$test_name" --exact --ignored --nocapture
done
echo 'PASS: native Secret Service roundtrip/delete and locked-provider fail-closed checks.'
SESSION
