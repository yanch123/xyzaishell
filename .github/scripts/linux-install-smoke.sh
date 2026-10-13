#!/usr/bin/env bash
# A package/linkage/Xvfb startup check, not interactive desktop acceptance.
set -euo pipefail
: "${PACKAGE_DIR:?PACKAGE_DIR is required}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
evidence="$RUNNER_TEMP/xyzaishell-smoke"
mkdir -p "$evidence"
exec > >(tee "$evidence/install.txt") 2>&1
cat /etc/os-release
uname -a
printf 'Source commit: %s\n' "$(git rev-parse HEAD)"
shopt -s nullglob
packages=("$PACKAGE_DIR"/*.deb)
[[ ${#packages[@]} == 1 ]] || { echo 'Expected exactly one .deb'; exit 1; }
(cd "$PACKAGE_DIR" && sha256sum --check SHA256SUMS-linux-x64.txt)
sudo apt-get update
sudo apt-get install --no-install-recommends -y xvfb xauth xdotool dbus-x11 "${packages[0]}"
dpkg-deb --info "${packages[0]}"
package_name=$(dpkg-deb --field "${packages[0]}" Package)
dpkg-query -W "$package_name" libwebkit2gtk-4.1-0 libgtk-3-0 || true
test -x /usr/bin/xyzaishell
dpkg -L "$package_name" | grep -E '/share/applications/[^/]+\.desktop$'
ldd /usr/bin/xyzaishell | tee "$evidence/linkage.txt"
if grep -q 'not found' "$evidence/linkage.txt"; then
  echo 'Missing runtime libraries'; exit 1
fi
# HOME/config are isolated; no real SSH or model account is configured.
export HOME="$RUNNER_TEMP/xyzaishell-smoke-home"
export XDG_CONFIG_HOME="$HOME/config" XDG_DATA_HOME="$HOME/data" XDG_CACHE_HOME="$HOME/cache"
export XDG_RUNTIME_DIR="$HOME/runtime"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export SMOKE_EVIDENCE="$evidence"
xvfb-run -a dbus-run-session -- bash -euo pipefail -c '
  /usr/bin/xyzaishell >"$SMOKE_EVIDENCE/startup.txt" 2>&1 &
  app_pid=$!
  trap '\''kill "$app_pid" 2>/dev/null || true; wait "$app_pid" 2>/dev/null || true'\'' EXIT
  window=""
  for attempt in $(seq 1 30); do
    kill -0 "$app_pid"
    window=$(xdotool search --onlyvisible --pid "$app_pid" 2>/dev/null || true)
    if [[ -n "$window" ]]; then break; fi
    sleep 1
  done
  [[ -n "$window" ]] || { echo "No visible application window"; exit 1; }
  sleep 5
  kill -0 "$app_pid"
  printf "PASS: installed application created a visible Xvfb window and stayed running.\n" > "$SMOKE_EVIDENCE/result.txt"
  printf "NOT TESTED: interactive behavior, real desktop sessions, rendering correctness, input methods, SSH, model calls, keyring, upgrade, uninstall.\n" >> "$SMOKE_EVIDENCE/result.txt"
'
cat "$evidence/result.txt"
