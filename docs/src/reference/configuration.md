# Configuration

`~/.config/herdr-deck/config.toml`. Every setting has a working default, so the file is optional
— herdr-deck is useful the moment it starts. Write a starter file with:

```sh
herdr-deck install
```

A **missing** config is normal. A **malformed** one is an error: unknown keys are rejected rather
than ignored, so a typo like `raise_windows` tells you instead of silently doing nothing.

## Top level

```toml
# Colour theme. Only "dark" today.
theme = "dark"

# How often to reconcile with herdr even when no event arrives, in milliseconds.
# Events are the fast path; this is the safety net that turns a dropped event into
# latency rather than a stuck deck. Clamped to a 250ms floor.
reconcile_interval_ms = 2000

# Talk to a named herdr session, the equivalent of `herdr --session work`.
# Omit for the default session.
herdr_session = "work"
```

## `[focus]`

```toml
[focus]
# Also raise the terminal window, not just switch herdr's pane.
raise_window = true

# Which terminal hosts herdr.
macos_bundle_id = "com.mitchellh.ghostty"
linux_app_id = "com.mitchellh.ghostty"

# Force a window-raise backend instead of detecting one.
# macos | hyprland | sway | kwin | x11 | unsupported
backend = "x11"

# Ask herdr to stamp a unique marker into its window title so the exact window
# can be found, rather than merely the right application.
use_title_marker = true
```

See [Window-raise backends](../focus/backends.md) for ids for other terminals.

## `[layout]` — pinning keys

By default every key shows an agent chosen dynamically, which is what makes herdr-deck useful
with no configuration. If you want specific keys pinned, replace the whole layout:

```toml
[layout]
keys = [
  # The Nth agent in attention order.
  { kind = "dynamic", rank = 0 },
  { kind = "dynamic", rank = 1 },

  # Always this agent, whatever it is doing. Use the stable terminal_id
  # (`herdr-deck status --json` prints it).
  { kind = "pinned_agent", terminal_id = "term_abc123" },

  # Always this workspace.
  { kind = "pinned_workspace", workspace_id = "w1" },

  # Fixed controls.
  { kind = "next_attention" },
  { kind = "mode_toggle" },
  { kind = "page_prev" },
  { kind = "page_next" },

  # Dial behaviour on a key, for hardware without encoders.
  { kind = "scrub", target = "attention", delta = 1 },

  { kind = "empty" },
]

dials = [
  { kind = "scrub", target = "agents" },
  { kind = "scrub", target = "workspaces" },
  { kind = "scrub", target = "tabs" },
  { kind = "scrub", target = "attention" },
  { kind = "unused" },
]
```

Scrub targets: `agents`, `workspaces`, `tabs`, `attention`.

A hand-written layout shorter than the hardware is padded with empty keys, so you cannot
accidentally leave keys unaddressable.

> Pinning is a trade. A pinned key always points at the same agent, even when a different one is
> screaming for attention. The dynamic default is usually the better choice — consider pinning
> only a couple of keys and leaving the rest dynamic.

## Environment variables

| Variable | Effect |
|---|---|
| `HERDR_SOCKET_PATH` | herdr's API socket, overriding discovery. |
| `HERDR_SESSION` | herdr session name. |
| `HERDR_DECK_SOCKET` | The daemon's frontend socket (read by the macOS plugin). |
| `HERDR_DECK_LOG` | Log filter, e.g. `debug` or `herdr_deckd=trace`. |
| `HERDR_DECKD_PATH` | Where `herdr-deck service` looks for the daemon binary. |

## Applying changes

```sh
herdr-deck service restart
```
