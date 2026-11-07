#!/bin/sh
# Package a built release binary the way the release workflow and the installers expect.
#   scripts/package.sh <target-triple> <path-to-ostler-binary> <out-dir>
# Produces, in <out-dir>:
#   ostler-v<version>-<target>.tar.gz          (contains ostler-v<version>-<target>/{ostler,LICENSE,README.md})
#   ostler-v<version>-<target>.tar.gz.sha256   (sha256 of the archive, `<hash>  <file>`)
set -eu
target="$1"; bin="$2"; out="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
[ -n "$version" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }
name="ostler-v${version}-${target}"
mkdir -p "$out"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
case "$target" in *windows*) exe=ostler.exe ;; *) exe=ostler ;; esac
cp "$bin" "$stage/$name/$exe"
chmod 755 "$stage/$name/$exe"
cp "$root/LICENSE" "$root/README.md" "$stage/$name/"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
if command -v sha256sum >/dev/null 2>&1; then h="$(sha256sum "$out/$name.tar.gz" | cut -d' ' -f1)"; else h="$(shasum -a 256 "$out/$name.tar.gz" | cut -d' ' -f1)"; fi
echo "$h  $name.tar.gz" > "$out/$name.tar.gz.sha256"
echo "$out/$name.tar.gz"
