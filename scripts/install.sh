#!/bin/sh
set -eu

repository="9MidhunPM/mcpd"
install_dir="${MCPD_INSTALL_DIR:-${HOME}/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  *)
    printf '%s\n' "mcpd prebuilt releases currently support Linux x86_64; use 'cargo install mcpd' on this platform." >&2
    exit 1
    ;;
esac

for command in curl sha256sum tar; do
  command -v "$command" >/dev/null 2>&1 || {
    printf '%s\n' "required command not found: $command" >&2
    exit 1
  }
done

version="${MCPD_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSL "https://api.github.com/repos/${repository}/releases/latest" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
fi
case "$version" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) printf '%s\n' "could not determine a valid mcpd release version" >&2; exit 1 ;;
esac

archive="mcpd-${version}-${target}.tar.gz"
base="https://github.com/${repository}/releases/download/${version}"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

curl -fsSL "${base}/${archive}" -o "${temporary}/${archive}"
curl -fsSL "${base}/checksums.txt" -o "${temporary}/checksums.txt"
(cd "$temporary" && sha256sum -c checksums.txt)
mkdir -p "$install_dir"
tar -C "$temporary" -xzf "${temporary}/${archive}"
install -m 0755 "${temporary}/mcpd" "${install_dir}/mcpd"
printf 'Installed mcpd %s to %s/mcpd\n' "$version" "$install_dir"
