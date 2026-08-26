#!/bin/sh
set -eu

repository="9MidhunPM/syncplane"
home_dir="${HOME:-}"
if [ -z "$home_dir" ]; then
  printf '%s\n' "HOME is not set; set SYNCPLANE_INSTALL_DIR explicitly." >&2
  exit 1
fi
install_dir="${SYNCPLANE_INSTALL_DIR:-${home_dir}/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  *)
    printf '%s\n' "syncplane prebuilt releases currently support Linux x86_64; use 'cargo install syncplane' on this platform." >&2
    exit 1
    ;;
esac

for command in curl sha256sum tar; do
  command -v "$command" >/dev/null 2>&1 || {
    printf '%s\n' "required command not found: $command" >&2
    exit 1
  }
done

version="${SYNCPLANE_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSL "https://api.github.com/repos/${repository}/releases/latest" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
fi
case "$version" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) printf '%s\n' "could not determine a valid syncplane release version" >&2; exit 1 ;;
esac

archive="syncplane-${version}-${target}.tar.gz"
base="https://github.com/${repository}/releases/download/${version}"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

curl -fsSL "${base}/${archive}" -o "${temporary}/${archive}"
curl -fsSL "${base}/checksums.txt" -o "${temporary}/checksums.txt"
if ! grep -F "  ${archive}" "${temporary}/checksums.txt" > "${temporary}/selected-checksum.txt"; then
  printf '%s\n' "release checksums do not contain ${archive}" >&2
  exit 1
fi
(cd "$temporary" && sha256sum -c selected-checksum.txt)

if [ -L "$install_dir/syncplane" ]; then
  printf '%s\n' "refusing to replace symlink: $install_dir/syncplane" >&2
  exit 1
fi
if [ -e "$install_dir/syncplane" ] && [ ! -f "$install_dir/syncplane" ]; then
  printf '%s\n' "refusing to replace non-regular file: $install_dir/syncplane" >&2
  exit 1
fi
mkdir -p "$install_dir"
tar -C "$temporary" -xzf "${temporary}/${archive}"
if [ ! -f "${temporary}/syncplane" ]; then
  printf '%s\n' "release archive does not contain syncplane" >&2
  exit 1
fi
install -m 0755 "${temporary}/syncplane" "${install_dir}/.syncplane.new"
mv -f "${install_dir}/.syncplane.new" "${install_dir}/syncplane"
printf 'Installed syncplane %s to %s/syncplane\n' "$version" "$install_dir"
case ":${PATH:-}:" in
  *":${install_dir}:"*) ;;
  *) printf '%s\n' "Add $install_dir to PATH to run syncplane from a new shell." >&2 ;;
esac
