#!/bin/sh
# Build a Claude Desktop extension (.mcpb): a zip the user double-clicks, with no terminal, Rust or JSON editing.
#   scripts/package-mcpb.sh <target-triple> <path-to-ostler-binary> <out-dir>
# Produces <out-dir>/ostler-v<version>-<target>.mcpb. Claude Desktop runs on macOS and Windows; Windows is not built yet.
set -eu
target="$1"; bin="$2"; out="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
[ -n "$version" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }
mkdir -p "$out"
out="$(cd "$out" && pwd)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/server"
cp "$bin" "$stage/server/ostler"
chmod 755 "$stage/server/ostler"
cp "$root/LICENSE" "$stage/"
sed "s/@VERSION@/$version/" "$root/packaging/mcpb/manifest.json" > "$stage/manifest.json"
file="$out/ostler-v${version}-${target}.mcpb"
rm -f "$file"
(cd "$stage" && zip -qr "$file" manifest.json LICENSE server)
echo "$file"
