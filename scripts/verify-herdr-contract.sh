#!/usr/bin/env bash
#
# Check herdr-deck's client against a real herdr, not against our idea of one.
#
# Two bugs shipped that every unit test agreed were fine, because the mock had been written from
# the same wrong reading of herdr's API as the client:
#
#   * `session.snapshot` returns `{"type": ..., "snapshot": {...}}`. The client read the result as
#     the snapshot. Nothing failed — every field has a default — so it got a valid, empty session:
#     no agents on the deck, and `doctor` reporting that herdr never sent a protocol version.
#   * `pane.agent_status_changed` has no unfiltered form; herdr requires a `pane_id` and rejects
#     the whole `events.subscribe` batch without one. Every key on the deck read
#     `herdr error: invalid_request`.
#
# Neither was reachable by any test that did not speak to herdr. This starts a real herdr and runs
# `herdr-deck doctor` against it, which exercises the snapshot decode and opens a live event
# subscription. herdr needs a terminal, so it is started under a pty.
#
# Usage:
#   scripts/verify-herdr-contract.sh            # uses `herdr` from PATH
#   HERDR=/path/to/herdr scripts/verify-herdr-contract.sh

set -euo pipefail

HERDR="${HERDR:-herdr}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v "$HERDR" >/dev/null 2>&1 && [ ! -x "$HERDR" ]; then
  echo "error: herdr not found (set HERDR=/path/to/herdr)" >&2
  exit 1
fi
herdr_bin="$(command -v "$HERDR" || echo "$HERDR")"

sandbox="$(mktemp -d)"
herdr_pid=""
cleanup() {
  [ -n "$herdr_pid" ] && kill "$herdr_pid" 2>/dev/null || true
  rm -rf "$sandbox"
}
trap cleanup EXIT

export XDG_CONFIG_HOME="$sandbox/config"
export XDG_STATE_HOME="$sandbox/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME" "$sandbox/work"

echo "==> starting herdr ($("$herdr_bin" --version))"
# herdr is a TUI and exits immediately without a terminal, so give it a pty. Nothing is read from
# it; we only want the API socket it serves.
python3 - "$herdr_bin" "$sandbox/work" <<'PY' &
import os, pty, sys
os.environ["TERM"] = "xterm-256color"
binary, workdir = sys.argv[1], sys.argv[2]
pid, _fd = pty.fork()
if pid == 0:
    os.chdir(workdir)
    os.execv(binary, [binary])
os.waitpid(pid, 0)
PY
herdr_pid=$!

socket="$XDG_CONFIG_HOME/herdr/herdr.sock"
for _ in $(seq 1 60); do
  [ -S "$socket" ] && break
  sleep 1
done
if [ ! -S "$socket" ]; then
  echo "error: herdr never created its socket at $socket" >&2
  exit 1
fi
echo "    socket at $socket"

echo "==> building herdr-deck"
(cd "$repo_root" && cargo build --quiet -p herdr-deck-cli)

echo "==> herdr-deck doctor, against that herdr"
set +e
HERDR_SOCKET_PATH="$socket" "$repo_root/target/debug/herdr-deck" doctor > "$sandbox/doctor.out" 2>&1
set -e
sed 's/^/    /' "$sandbox/doctor.out"

# doctor's exit code covers the whole machine — no deck attached and no window-raise backend are
# expected on a CI runner and are not what this checks. Only the herdr-facing checks matter here.
fail=0
for check in "herdr socket" "herdr protocol" "herdr events"; do
  line="$(grep -F "] $check:" "$sandbox/doctor.out" || true)"
  if [ -z "$line" ]; then
    echo "error: doctor never reported \`$check\`" >&2
    fail=1
  elif ! printf '%s' "$line" | grep -q '^\[ok'; then
    echo "error: \`$check\` did not pass against a real herdr" >&2
    fail=1
  fi
done

# A protocol version herdr did report, decoded from the right level of the response. The empty
# case is the exact symptom of reading the snapshot one level too high.
if grep -qF "did not report a protocol version" "$sandbox/doctor.out"; then
  echo "error: the snapshot decoded without a protocol version — the response shape has moved" >&2
  fail=1
fi

[ "$fail" -eq 0 ] || exit 1
echo "OK: herdr-deck's client agrees with a real herdr."
