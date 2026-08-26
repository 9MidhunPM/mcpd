#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

mkdir -p "$temporary/package" "$temporary/bin" "$temporary/install"
printf '%s\n' '#!/bin/sh' "printf '%s\\n' 'syncplane 1.0.0'" > "$temporary/package/syncplane"
chmod 755 "$temporary/package/syncplane"
tar -C "$temporary/package" -czf "$temporary/syncplane-v1.0.0-x86_64-unknown-linux-gnu.tar.gz" syncplane
(cd "$temporary" && sha256sum syncplane-v1.0.0-x86_64-unknown-linux-gnu.tar.gz > checksums.txt)

printf '%s\n' \
  '#!/bin/sh' \
  'set -eu' \
  "destination=''" \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    -o) destination=$2; shift 2 ;;' \
  '    *) url=$1; shift ;;' \
  '  esac' \
  'done' \
  'case "$url" in' \
  '  *checksums.txt) cp "$SYNCPLANE_TEST_CHECKSUMS" "$destination" ;;' \
  '  *) cp "$SYNCPLANE_TEST_ARCHIVE" "$destination" ;;' \
  'esac' > "$temporary/bin/curl"
chmod 755 "$temporary/bin/curl"

PATH="$temporary/bin:$PATH" \
  HOME="$temporary/home" \
  SYNCPLANE_INSTALL_DIR="$temporary/install" \
  SYNCPLANE_VERSION=v1.0.0 \
  SYNCPLANE_TEST_ARCHIVE="$temporary/syncplane-v1.0.0-x86_64-unknown-linux-gnu.tar.gz" \
  SYNCPLANE_TEST_CHECKSUMS="$temporary/checksums.txt" \
  sh "$root/scripts/install.sh"

test "$("$temporary/install/syncplane" --version)" = 'syncplane 1.0.0'

mv "$temporary/install/syncplane" "$temporary/installed-binary"
ln -s "$temporary/elsewhere" "$temporary/install/syncplane"
if PATH="$temporary/bin:$PATH" HOME="$temporary/home" SYNCPLANE_INSTALL_DIR="$temporary/install" SYNCPLANE_VERSION=v1.0.0 SYNCPLANE_TEST_ARCHIVE="$temporary/syncplane-v1.0.0-x86_64-unknown-linux-gnu.tar.gz" SYNCPLANE_TEST_CHECKSUMS="$temporary/checksums.txt" sh "$root/scripts/install.sh" >/dev/null 2>&1; then
  printf '%s\n' 'installer accepted a symlink target' >&2
  exit 1
fi
