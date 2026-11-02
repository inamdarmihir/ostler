#!/bin/sh
# Ostler installer: downloads a prebuilt binary, verifies its checksum, installs it.
#   curl -fsSL https://inamdarmihir.github.io/ostler/install.sh | sh
#
# Environment:
#   OSTLER_VERSION       version to install, e.g. 0.2.0 (default: the latest release)
#   OSTLER_INSTALL_DIR   where to put the binary (default: ~/.local/bin)
#   OSTLER_RELEASE_BASE  release URL base (default: the GitHub releases page; used for testing)
#
# It never edits your shell profile or your agent's config. It prints what to run next.
set -eu

REPO="inamdarmihir/ostler"
BASE="${OSTLER_RELEASE_BASE:-https://github.com/$REPO/releases}"
DEST="${OSTLER_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'ostler install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required but was not found"; }

need curl
need tar
need uname

# --- which build?
os="$(uname -s)"; arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)                 target="aarch64-apple-darwin" ;;
  Darwin/x86_64)                target="x86_64-apple-darwin" ;;
  Linux/x86_64)                 target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64|Linux/arm64)    target="aarch64-unknown-linux-gnu" ;;
  *) die "no prebuilt binary for $os/$arch. Build from source instead: cargo install ostler" ;;
esac

# --- which version?
version="${OSTLER_VERSION:-}"
if [ -z "$version" ]; then
  # /releases/latest redirects to /releases/tag/vX.Y.Z; read the final URL rather than calling the API.
  url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$BASE/latest")" || die "could not look up the latest release (set OSTLER_VERSION to skip this)"
  version="${url##*/v}"
  case "$version" in ""|*/*) die "could not work out the latest version from $url (set OSTLER_VERSION)";; esac
fi
version="${version#v}"

name="ostler-v${version}-${target}"
archive="$name.tar.gz"
dl="$BASE/download/v${version}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Installing ostler $version for $target"
curl -fsSL "$dl/$archive" -o "$tmp/$archive" || die "download failed: $dl/$archive (is $version a published release for $target?)"
curl -fsSL "$dl/$archive.sha256" -o "$tmp/$archive.sha256" || die "checksum file missing: $dl/$archive.sha256"

# --- verify before anything is extracted or run
want="$(cut -d' ' -f1 "$tmp/$archive.sha256")"
if command -v sha256sum >/dev/null 2>&1; then got="$(sha256sum "$tmp/$archive" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then got="$(shasum -a 256 "$tmp/$archive" | cut -d' ' -f1)"
else die "need sha256sum or shasum to verify the download"; fi
[ "$want" = "$got" ] || die "checksum mismatch (expected $want, got $got). Not installing."

tar -xzf "$tmp/$archive" -C "$tmp"
[ -f "$tmp/$name/ostler" ] || die "the archive did not contain $name/ostler"
mkdir -p "$DEST"
cp "$tmp/$name/ostler" "$DEST/ostler.new"
chmod 755 "$DEST/ostler.new"
mv -f "$DEST/ostler.new" "$DEST/ostler"   # atomic replace: safe if an old copy is running

say "Installed $DEST/ostler"
"$DEST/ostler" --version || die "the installed binary does not run on this machine"

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) say ""
     say "$DEST is not on your PATH. Add it, for example:"
     say "  echo 'export PATH=\"$DEST:\$PATH\"' >> ~/.zshrc   # or ~/.bashrc"
     ;;
esac

say ""
say "Next:"
say "  ostler doctor                 # checks Chrome (needed) and Qdrant (optional)"
say "  ostler setup claude --apply   # connect Claude Code (also: codex, cursor, vscode)"
