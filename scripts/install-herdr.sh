#!/usr/bin/env bash
#
# Put the herdr release binary this plugin claims to support onto PATH.
#
# The version is read from `min_herdr_version` in herdr-plugin.toml, so CI tests the compatibility
# floor we actually publish. Raising that field changes what gets tested, which is the point —
# there is no second place to keep in step.
#
# herdr publishes bare executables, not archives; the names come from its release workflow.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

version="$(sed -n 's/^min_herdr_version = "\(.*\)"/\1/p' "$repo_root/herdr-plugin.toml")"
if [ -z "$version" ]; then
  echo "error: herdr-plugin.toml does not declare min_herdr_version" >&2
  exit 1
fi

case "$(uname -s)" in
  Linux)  os=linux ;;
  Darwin) os=macos ;;
  *) echo "error: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64)  arch=x86_64 ;;
  arm64|aarch64) arch=aarch64 ;;
  *) echo "error: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

asset="herdr-${os}-${arch}"
url="https://github.com/herdrdev/herdr/releases/download/v${version}/${asset}"

dest="${HERDR_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dest"

echo "downloading $asset (herdr $version)"
# -f matters: without it a renamed or missing asset returns a 404 body that curl writes to the
# destination quite happily, and the failure surfaces later as a confusing exec error.
curl -fsSL --retry 3 --retry-delay 2 -o "$dest/herdr" "$url"
chmod +x "$dest/herdr"

if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$dest" >> "$GITHUB_PATH"
fi
export PATH="$dest:$PATH"

"$dest/herdr" --version
