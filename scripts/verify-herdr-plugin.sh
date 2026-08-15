#!/usr/bin/env bash
#
# Check `herdr-plugin.toml` against a real herdr.
#
# This file is consumed by a *different program*, so nothing in this repository's own toolchain
# reads it — and it shipped broken: every action used `name` where herdr requires `title`, so
# `herdr plugin install` died on the first `[[actions]]` block before doing anything. Cargo
# validates `Cargo.toml`, `streamdeck validate` validates the Stream Deck manifest; this one had
# no checker at all.
#
# `crates/herdr-deck-cli/tests/plugin_manifest.rs` covers the same ground by mirroring herdr's
# structs, which is fast and needs nothing installed — but it can only be as right as our reading
# of herdr's source. This script is the other half: it runs the actual herdr binary and believes
# whatever it says. If herdr changes its schema, this fails and the unit test does not.
#
# Usage:
#   scripts/verify-herdr-plugin.sh            # uses `herdr` from PATH
#   HERDR=/path/to/herdr scripts/verify-herdr-plugin.sh
#
# No herdr server needs to be running: herdr's plugin commands fall back to a registry-on-disk
# path when they cannot reach a session.

set -euo pipefail

HERDR="${HERDR:-herdr}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v "$HERDR" >/dev/null 2>&1 && [ ! -x "$HERDR" ]; then
  echo "error: herdr not found (set HERDR=/path/to/herdr)" >&2
  exit 1
fi

# Linking writes to herdr's plugin registry. Point herdr at a throwaway config so running this on
# a developer machine cannot disturb the plugins they actually have installed. XDG_CONFIG_HOME
# takes priority over the platform directory on macOS too, so this is not Linux-only.
sandbox="$(mktemp -d)"
trap 'rm -rf "$sandbox"' EXIT
export XDG_CONFIG_HOME="$sandbox/config"
export XDG_STATE_HOME="$sandbox/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"

echo "herdr: $("$HERDR" --version)"

echo "==> herdr parses herdr-plugin.toml"
"$HERDR" plugin link "$repo_root" >/dev/null

echo "==> the actions we declare are the actions herdr registered"
"$HERDR" plugin list --json > "$sandbox/list.json"
python3 - "$sandbox/list.json" "$repo_root/herdr-plugin.toml" <<'PY'
import json, re, sys

listing, manifest_path = sys.argv[1], sys.argv[2]

with open(listing) as handle:
    plugins = json.load(handle)["result"]["plugins"]

wanted = "sneakytowelsuit.herdr-deck"
plugin = next((p for p in plugins if p["plugin_id"] == wanted), None)
if plugin is None:
    sys.exit(f"herdr did not register {wanted}; it has: {[p['plugin_id'] for p in plugins]}")

# Compare against the ids in the file rather than a list hard-coded here, so adding an action
# without herdr accepting it is a failure rather than something this check quietly ignores.
declared = set(re.findall(r'^id = "([^"]+)"', open(manifest_path).read(), re.M))
declared.discard(plugin["plugin_id"])
registered = {action["id"] for action in plugin["actions"]}
if declared != registered:
    sys.exit(f"actions declared {sorted(declared)} but herdr registered {sorted(registered)}")

for action in plugin["actions"]:
    if not action.get("title", "").strip():
        sys.exit(f"action {action['id']} reached herdr with a blank title")
    if not action.get("contexts"):
        # herdr defaults `contexts` to empty, and an action in no context is one nobody can
        # reach from the TUI — it installs cleanly and is simply invisible.
        sys.exit(f"action {action['id']} is registered in no context, so nothing can invoke it")

print(f"  {len(registered)} actions: {' '.join(sorted(registered))}")
PY

echo "==> the commands we tell people to run are commands herdr understands"
# The install guide shipped `herdr plugin action <plugin-id> install`. herdr's dispatcher only
# knows `list` and `invoke`, so anything else prints the help text and exits — doing nothing,
# quietly enough to look like it worked. A user hit this during a real install. Prose is not
# checked by anything, so it is checked here.
python3 - "$repo_root" <<'PY'
import pathlib, re, sys

root = pathlib.Path(sys.argv[1])
docs = list((root / "docs" / "src").rglob("*.md")) + [root / "README.md"]

ids = set(re.findall(r'^id = "([^"]+)"', (root / "herdr-plugin.toml").read_text(), re.M))
ids.discard("sneakytowelsuit.herdr-deck")

problems = []
for path in docs:
    for n, line in enumerate(path.read_text().splitlines(), 1):
        stripped = line.strip()
        if not stripped.startswith("herdr plugin action"):
            continue
        words = stripped.split()
        verb = words[3] if len(words) > 3 else ""
        where = f"{path.relative_to(root)}:{n}"
        if verb not in ("list", "invoke"):
            problems.append(f"{where}: `{verb or '(nothing)'}` — herdr only knows `list` and `invoke`")
        elif verb == "invoke":
            action = words[4] if len(words) > 4 else ""
            bare = action.rsplit(".", 1)[-1]
            if bare not in ids:
                problems.append(f"{where}: invokes `{action}`, which herdr-plugin.toml does not define")

if problems:
    print("documented commands herdr would reject:")
    for p in problems:
        print("  " + p)
    raise SystemExit(1)
print("  every documented `herdr plugin action` command is well formed")
PY

echo "==> the check can actually fail (negative control)"
# A gate that passes unconditionally is worse than no gate, because it reads as coverage. Feed
# herdr the exact bug that shipped and require it to reject it.
broken="$sandbox/broken"
mkdir -p "$broken"
sed 's/^title = /name = /' "$repo_root/herdr-plugin.toml" > "$broken/herdr-plugin.toml"
if "$HERDR" plugin link "$broken" >"$sandbox/broken.out" 2>&1; then
  echo "error: herdr accepted a manifest using \`name\` instead of \`title\`." >&2
  echo "This check cannot detect the bug it exists to catch — herdr's schema has changed." >&2
  exit 1
fi
if ! grep -q "missing field .title." "$sandbox/broken.out"; then
  echo "warning: the broken manifest was rejected, but not for the expected reason:" >&2
  sed 's/^/  /' "$sandbox/broken.out" >&2
fi

echo "OK: herdr $("$HERDR" --version | awk '{print $2}') accepts this plugin."
