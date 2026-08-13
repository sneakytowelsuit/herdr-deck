# Troubleshooting

Start here:

```sh
herdr-deck doctor
```

It checks every failure mode below and prints the fix for each one.

## Every key says "herdr not running"

The daemon cannot reach herdr's socket.

- Is herdr running? `herdr status`
- Are you using a named session? The daemon defaults to the default session. Point it at yours
  with `herdr_session = "work"` in config, or `herdr-deckd --session work`.
- `herdr-deck doctor` prints the exact socket path it tried and how it chose it.

The deck reconnects on its own once herdr is back — no restart needed.

## The deck is completely dark

Nothing is driving the hardware.

**macOS.** Is the plugin installed and placed? Open the Stream Deck app and check that **herdr
Agent** is on your keys. Then check the daemon is up: `herdr-deck service status`.

**Linux.** Almost always the udev rule. Without it the daemon cannot open the device and logs
"no Stream Deck found". See [Install](../getting-started/install.md#linux), and remember to
unplug and replug afterwards.

## Keys work, but the terminal never comes forward

herdr is switching panes but the window raise is failing. `herdr-deck doctor` says which backend
it picked.

- **`no usable backend`** — most likely [GNOME on Wayland](../focus/gnome-wayland.md).
- **`window tools: none of wmctrl, xdotool are installed`** — install one.
- **Backend fine, still nothing** — herdr-deck is probably looking for the wrong terminal. Set
  `focus.macos_bundle_id` / `focus.linux_app_id`; see
  [Window-raise backends](../focus/backends.md#telling-herdr-deck-about-your-terminal).

## An agent sits at a prompt but its key stays dim

herdr has not classified it as `blocked`. herdr-deck shows what herdr believes, so diagnose at
the source:

```sh
herdr agent explain <target>
```

herdr only marks `blocked` when the screen matches a known approval or question UI; unrecognised
prompts fall back to `idle`. See [`blocked` and `done`](../concepts/attention.md#an-important-caveat-about-detection).

## Tiles lag behind

Events are the fast path and a timer is the safety net, so the worst case is your
`reconcile_interval_ms` (2s by default). If everything is consistently slow, check
`herdr-deck doctor` for a protocol mismatch — a herdr that speaks a different protocol may not be
sending the events herdr-deck subscribes to.

## `another herdr-deckd is already listening`

One is running. `herdr-deck service status`, then `herdr-deck service restart`.

If it crashed and left a stale socket, herdr-deck reclaims it automatically — it only refuses when
something is genuinely listening.

## The plugin logs `protocol_mismatch`

The Stream Deck plugin and the daemon are different versions. They install through different
mechanisms, so this drifts. Update whichever is older and restart both.

## My config changes do nothing

- Restart the daemon: `herdr-deck service restart`.
- Run `herdr-deck doctor` — a malformed config is reported as a `FAIL`, including which key it
  choked on. Unknown keys are rejected deliberately, so a typo is loud rather than silent.

## Getting more detail

```sh
herdr-deck service stop
herdr-deckd --log debug
```

macOS plugin logs are under `~/Library/Logs/ElgatoStreamDeck/`.
