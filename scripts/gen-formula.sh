#!/bin/sh
# Print the Homebrew formula for a release, built from that release's SHA256SUMS.
#   scripts/gen-formula.sh <version> <path-to-SHA256SUMS>   > ostler.rb
# The release workflow runs this and pushes the result to the tap. It needs no network.
set -eu
version="${1#v}"; sums="$2"
[ -n "$version" ] && [ -f "$sums" ] || { echo "usage: $0 <version> <SHA256SUMS>" >&2; exit 1; }
sha() { # sha <target>
  s="$(awk -v f="ostler-v${version}-$1.tar.gz" '$2 == f { print $1 }' "$sums")"
  case "$s" in ????????????????????????????????????????????????????????????????) printf '%s' "$s" ;;
    *) echo "gen-formula: no valid sha256 for $1 in $sums" >&2; exit 1 ;; esac
}
arm_mac="$(sha aarch64-apple-darwin)";      x64_mac="$(sha x86_64-apple-darwin)"
arm_lin="$(sha aarch64-unknown-linux-gnu)"; x64_lin="$(sha x86_64-unknown-linux-gnu)"
base="https://github.com/inamdarmihir/ostler/releases/download/v${version}/ostler-v${version}"
cat <<RUBY
class Ostler < Formula
  desc "Open-source MCP browser for AI agents: reading, cited research, web tasks, forms read back"
  homepage "https://inamdarmihir.github.io/ostler/"
  version "${version}"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "${base}-aarch64-apple-darwin.tar.gz"
      sha256 "${arm_mac}"
    else
      url "${base}-x86_64-apple-darwin.tar.gz"
      sha256 "${x64_mac}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "${base}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${arm_lin}"
    else
      url "${base}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${x64_lin}"
    end
  end

  def install
    bin.install "ostler"
  end

  def caveats
    <<~EOS
      Ostler needs Chrome or Chromium. Check your setup with:  ostler doctor
      Connect an agent with:  ostler setup claude --apply
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ostler --version")
  end
end
RUBY
