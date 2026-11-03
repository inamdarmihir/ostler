#!/bin/sh
# Download a pinned Qdrant release binary, verify its sha256, and unpack it. Used by CI.
#   scripts/ci-install-qdrant.sh <version> <sha256> <dest-dir>
# Prints the path of the verified binary on stdout. Fails (and installs nothing) on a checksum mismatch.
set -eu
[ "$#" -eq 3 ] || { echo "usage: $0 <version> <sha256> <dest-dir>" >&2; exit 2; }
version="$1"; want="$2"; dest="$3"
asset="qdrant-x86_64-unknown-linux-gnu.tar.gz"
url="https://github.com/qdrant/qdrant/releases/download/${version}/${asset}"
mkdir -p "$dest"
curl -sSfL --retry 5 --retry-delay 3 -o "$dest/$asset" "$url"
got="$(sha256sum "$dest/$asset" | cut -d' ' -f1)"
if [ "$got" != "$want" ]; then
  echo "sha256 mismatch for $asset ($version): expected $want, got $got" >&2
  rm -f "$dest/$asset"
  exit 1
fi
tar -xzf "$dest/$asset" -C "$dest" qdrant
chmod +x "$dest/qdrant"
echo "$dest/qdrant"
