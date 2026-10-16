#!/usr/bin/env bash
# Isolated historical compatibility runtime, NEVER a replacement system shell.
# Source and all 46 official GNU patches are hash pinned before extraction/build.
# Usage: bash .github/scripts/build-bash-4.2.sh /absolute/private-build-directory
set -euo pipefail
[[ $# == 1 && $1 == /* ]] || { echo 'Provide one absolute build directory.' >&2; exit 2; }
root=$1
[[ ! -e "$root" ]] || { echo 'Build directory must not already exist.' >&2; exit 1; }
for tool in curl sha256sum tar patch make gcc bison; do
  command -v "$tool" >/dev/null || { echo "Missing prerequisite: $tool" >&2; exit 1; }
done
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
manifest="$script_dir/fixtures/bash-4.2.46/SHA256SUMS"
[[ $(wc -l < "$manifest") == 47 ]] || { echo 'Incomplete source checksum manifest.' >&2; exit 1; }
mkdir -p "$root/patches" "$root/bin"
cd "$root"
fetch() {
  local url=$1 relative=$2
  if [[ -n ${XYZ_BASH42_SOURCE_CACHE:-} ]]; then
    # A cache is only transport: every byte is still checked against pins below.
    cp -- "$XYZ_BASH42_SOURCE_CACHE/$relative" "$relative"
  else
    curl --fail --show-error --silent --location --proto '=https' --tlsv1.2 \
      "$url" -o "$relative"
  fi
}
fetch https://ftp.gnu.org/gnu/bash/bash-4.2.tar.gz bash-4.2.tar.gz
for number in $(seq -w 1 46); do
  patch_name="bash42-0$number"
  fetch "https://ftp.gnu.org/gnu/bash/bash-4.2-patches/$patch_name" "patches/$patch_name"
done
sha256sum --check --strict "$manifest"
tar -xzf bash-4.2.tar.gz
for patch_file in patches/bash42-*; do
  patch --batch --forward --directory=bash-4.2 -p0 < "$patch_file"
done
cd bash-4.2
# GNU89 is required for old configure probes on modern GCC; otherwise job
# control can be silently misdetected. Keep common-symbol semantics as well.
CFLAGS='-O2 -std=gnu89 -fcommon' ./configure --without-bash-malloc > "$root/configure.log" 2>&1
grep -Eq '^#define JOB_CONTROL 1$' config.h || { echo 'Bash fixture lacks real job control.' >&2; exit 1; }
make -j2 > "$root/make.log" 2>&1
install -m 755 bash "$root/bin/bash"
version=$(BASH_ENV= "$root/bin/bash" --noprofile --norc -c 'printf "%s" "$BASH_VERSION"')
[[ "$version" == '4.2.46('* ]] || { echo "Wrong runtime built: $version" >&2; exit 1; }
BASH_ENV= "$root/bin/bash" --noprofile --norc -c 'set -m; [[ $- == *m* ]]' || {
  echo 'Built Bash failed runtime job-control check.' >&2; exit 1;
}
printf 'Verified real Bash %s at %s/bin/bash\n' "$version" "$root"
