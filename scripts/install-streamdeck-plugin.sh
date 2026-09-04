#!/usr/bin/env bash
#
# Build the Stream Deck plugin and hand it to the Stream Deck app.
#
# Only macOS needs this. Elgato's desktop app runs on macOS and Windows only, and it is the thing
# that talks to the hardware there — herdr-deck's plugin is a thin shim that draws what the daemon
# sends. On Linux there is no app: `herdr-deckd` drives the deck over USB HID itself, so there is
# nothing to install and this exits happily.
#
# `streamdeck link` symlinks the plugin rather than copying it, which is what makes updates work:
# `herdr plugin install` rebuilds inside herdr's managed checkout, and that checkout keeps a stable
# path, so the link keeps pointing at the freshly built code. Reinstalling is the whole update.
#
# Best effort by design. A failure here must not abort the install and leave the user with no
# daemon either — the daemon is the half that does the work, and a deck that draws nothing is
# something `herdr-deck doctor` will tell them about in plain words. So every failure below warns
# loudly and exits 0.

set -uo pipefail

plugin_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/plugin"
uuid="com.sneakytowelsuit.herdr-deck"
bundle="$uuid.sdPlugin"

warn() {
  echo "warning: $*" >&2
  echo "         The herdr-deck daemon is installed and working; only the Stream Deck app side" >&2
  echo "         was skipped. Run \`herdr-deck doctor\` to see what is missing." >&2
  exit 0
}

if [ "$(uname -s)" != "Darwin" ]; then
  echo "Linux: herdr-deckd drives the deck directly, no Stream Deck app to install into."
  exit 0
fi

if ! command -v npm >/dev/null 2>&1; then
  warn "npm was not found, so the Stream Deck plugin could not be built.
         Install Node 20 or newer and re-run \`herdr plugin install sneakytowelsuit/herdr-deck --yes\`."
fi

if [ ! -d "/Applications/Elgato Stream Deck.app" ]; then
  warn "the Elgato Stream Deck app does not appear to be installed.
         Install it from elgato.com, then re-run the install to link the plugin."
fi

cd "$plugin_dir" || warn "the plugin directory is missing from this checkout"

echo "==> building the Stream Deck plugin"
npm ci --silent || warn "npm ci failed; the Stream Deck plugin was not built"
npm run build --silent || warn "the plugin build failed"

# Linked, not copied: a copy would go stale the moment the daemon is rebuilt, and the user would
# have no way to tell which version the app was running.
echo "==> linking it into the Stream Deck app"
npx --yes @elgato/cli@latest link "$bundle" || warn "\`streamdeck link\` failed"

# The app only picks up a changed plugin when the plugin is restarted. Skipping this is how an
# install looks like it worked and changes nothing until the next reboot.
npx --yes @elgato/cli@latest restart "$uuid" >/dev/null 2>&1 || true

echo "OK: the Stream Deck plugin is linked. Drag \"herdr Agent\" onto your keys and \"herdr Dial\""
echo "    onto the dials — they are two separate actions."
