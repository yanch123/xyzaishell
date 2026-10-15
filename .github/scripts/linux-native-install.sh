#!/usr/bin/env bash
# Disposable CI installed-app UI/IPC + reinstall/remove preservation checks.
# Same-version reinstall is deliberately not claimed as a version upgrade.
set -euo pipefail
umask 077
: "${PACKAGE_DIR:?PACKAGE_DIR is required}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${TAURI_DRIVER:?Set up the pinned native driver first}"
[[ ${CI:-} == true && $(uname -s) == Linux ]] || { echo 'Disposable Linux CI only' >&2; exit 1; }
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
evidence="$RUNNER_TEMP/xyzaishell-native-linux"
upgrade=false
if [[ -n ${UPGRADE_BASELINE_DIR:-} ]]; then
  upgrade=true
  evidence="$evidence/version-upgrade"
fi
mkdir -p "$evidence"
exec > >(tee "$evidence/install.txt") 2>&1
fixture=$(mktemp -d /tmp/xyz-native.XXXXXXXX)
package_name=''
cleanup() {
  local result=$?
  trap - EXIT
  if [[ -n "$package_name" ]]; then
    sudo apt-get remove -y "$package_name" || result=1
    [[ ! -e /usr/bin/xyzaishell ]] || result=1
    if [[ -f "$fixture/final.sha256" ]]; then
      sha256sum --check "$fixture/final.sha256" || result=1
    fi
  fi
  rm -rf -- "$fixture" || result=1
  [[ ! -e "$fixture" ]] || result=1
  if [[ $result -ne 0 ]]; then
    echo 'Native acceptance or disposable fixture cleanup failed.' >&2
  else
    echo 'PASS: disposable application data and installed-test fixture removed.'
  fi
  exit "$result"
}
trap cleanup EXIT
printf 'synthetic disposable fixture\n' > "$fixture/.xyz-native-fixture"
export HOME="$fixture/home" XDG_CONFIG_HOME="$fixture/config" XDG_DATA_HOME="$fixture/data"
export XDG_CACHE_HOME="$fixture/cache" XDG_RUNTIME_DIR="$fixture/runtime"
export XDG_STATE_HOME="$fixture/state"
unset DBUS_SESSION_BUS_ADDRESS DBUS_SESSION_BUS_PID GNOME_KEYRING_CONTROL SSH_AUTH_SOCK XYZAI_MODEL_CONFIG || true
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR" "$XDG_STATE_HOME"
cat /etc/os-release
uname -a
printf 'Source commit: %s\n' "$(git -C "$repo" rev-parse HEAD)"
shopt -s nullglob
packages=("$PACKAGE_DIR"/*.deb)
[[ ${#packages[@]} == 1 ]] || { echo 'Expected exactly one DEB' >&2; exit 1; }
(cd "$PACKAGE_DIR" && sha256sum --check SHA256SUMS-linux-x64.txt)
candidate_name=$(dpkg-deb --field "${packages[0]}" Package)
[[ "$candidate_name" == xyzaishell ]] || { echo 'Unexpected package name' >&2; exit 1; }
(cd "$repo" && python3 .github/scripts/artifacts.py verify-package --platform linux-x64 --installers "$PACKAGE_DIR")
package_name=$candidate_name
candidate_version=$(cd "$repo" && python3 .github/scripts/artifacts.py check-version)
run_ui() {
  local phase=$1 label=$2 version=$3
  timeout --kill-after=10s 240s xvfb-run -a dbus-run-session -- \
    python3 "$repo/.github/scripts/native-ui.py" --application /usr/bin/xyzaishell \
      --fixture "$fixture" --evidence "$evidence/$label" --phase "$phase" --application-version "$version"
}
fingerprint() {
  sha256sum "$XDG_CONFIG_HOME/xyzaishell/"{model.toml,connections.toml} \
    "$XDG_STATE_HOME/xyzaishell/history.sqlite3"
}
if $upgrade; then
  baseline=$(cd "$repo" && python3 .github/scripts/upgrade_baseline.py verify \
    --platform linux-x64 --directory "$UPGRADE_BASELINE_DIR")
  sudo apt-get install --no-install-recommends -y "$baseline"
  [[ $(dpkg-query -W -f='${Version}' "$package_name") == 0.1.2 ]]
  (cd "$repo" && python3 .github/scripts/upgrade_baseline.py verify \
    --platform linux-x64 --directory "$UPGRADE_BASELINE_DIR" --application /usr/bin/xyzaishell)
  run_ui seed baseline-v0.1.2-install-and-restart 0.1.2
  fingerprint > "$fixture/before-upgrade.sha256"
fi
# No uninstall or data reset between the old application and this installation.
sudo apt-get install --no-install-recommends -y "${packages[0]}"
[[ $(dpkg-query -W -f='${Version}' "$package_name") == "$candidate_version" ]]
if $upgrade; then sha256sum --check "$fixture/before-upgrade.sha256"; fi
test -x /usr/bin/xyzaishell
dpkg-query -W "$package_name" 'libwebkit2gtk-4.1-*' 'libgtk-3-*' | tee "$evidence/runtime.txt"
dpkg -L "$package_name" | grep -E '/share/applications/[^/]+\.desktop$'
for resource in THIRD-PARTY-NOTICES.txt RUST-STANDARD-LIBRARY.html runtime-inventory.json sources/option-ext-0.2.0.crate; do
  path=$(dpkg -L "$package_name" | grep -F "/third-party/$resource")
  [[ -f "$path" && -s "$path" ]] || { echo "Missing installed notice/source: $resource" >&2; exit 1; }
  sha256sum "$path" >> "$evidence/installed-notices.sha256"
done
ldd /usr/bin/xyzaishell | tee "$evidence/linkage.txt"
! grep -q 'not found' "$evidence/linkage.txt"
if $upgrade; then
  run_ui upgrade upgraded-v0.1.3 "$candidate_version"
else
  run_ui seed first-install-and-restart "$candidate_version"
  fingerprint > "$fixture/before-reinstall.sha256"
  sudo apt-get install --reinstall --no-install-recommends -y "${packages[0]}"
  sha256sum --check "$fixture/before-reinstall.sha256"
  run_ui preserved same-version-reinstall "$candidate_version"
  fingerprint > "$fixture/before-uninstall.sha256"
  sudo apt-get remove -y "$package_name"
  test ! -e /usr/bin/xyzaishell
  sha256sum --check "$fixture/before-uninstall.sha256"
  sudo apt-get install --no-install-recommends -y "${packages[0]}"
  sha256sum --check "$fixture/before-uninstall.sha256"
  run_ui preserved uninstall-and-reinstall "$candidate_version"
fi
fingerprint > "$fixture/final.sha256"
if $upgrade; then
  printf '%s\n' 'PASS: pinned v0.1.2 installed and seeded through real UI; direct v0.1.3 upgrade retained private profile/model bytes, history rows, text and stable IDs.' > "$evidence/result.txt"
else
printf '%s\n' \
  'PASS: actual DEB installed; real native DOM/IPC profile/chat persistence and consent verified.' \
  'PASS: same-version reinstall and uninstall/reinstall preserved config hashes and restored text.' \
  'NOT TESTED: version upgrade, full desktop/X11/Wayland/IME/clipboard, live provider, SSH through UI.' \
  > "$evidence/result.txt"
fi
cat "$evidence/result.txt"
