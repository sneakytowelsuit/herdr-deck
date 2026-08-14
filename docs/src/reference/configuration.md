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

  # One herdr command, spelled out. The escape hatch for anything the derived
  # layout does not offer.
  { kind = "command", command = { verb = "focus_tab", tab_id = "w1:t2" } },

  # Close whichever pane herdr says is focused. Takes no arguments on purpose;
  # see below.
  { kind = "close_pane" },

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

Command verbs:

| Verb | Argument | What it does |
|---|---|---|
| `focus_pane` | `pane_id` | Take me to that pane. |
| `focus_workspace` | `workspace_id` | Take me to that workspace. |
| `focus_tab` | `tab_id` | Take me to that tab. |
| `move_pane_focus` | `direction` = `left` \| `right` \| `up` \| `down` | Move one pane that way. |
| `zoom_pane` | `zoom` = `on` \| `off` | Fill the tab with the focused pane, or put it back. |
| `split_pane` | `direction` = `right` \| `down` | New shell beside or below, and focus it. |
| `close_pane` | `pane_id` | **Destructive.** Prefer the `close_pane` *binding* below. |

A hand-written layout shorter than the hardware is padded with empty keys, so you cannot
accidentally leave keys unaddressable.

> A command that could destroy work is never issued by a tap. It moves to a hold, the key wears
> `hold` on its face, and tapping it says so rather than going quiet. The guard applies to the
> command, not to the key, so a hand-written layout cannot opt out of it.

## Pane control

Decks with 24 keys or more get [a pane cluster by
default](../getting-started/default-layout.md#pane-control-on-large-decks). Smaller ones keep
every key for agents, so this is how you ask for one:

```toml
[layout]
keys = [
  { kind = "dynamic", rank = 0 },
  { kind = "dynamic", rank = 1 },
  { kind = "dynamic", rank = 2 },
  { kind = "dynamic", rank = 3 },
  { kind = "command", command = { verb = "move_pane_focus", direction = "left" } },
  { kind = "command", command = { verb = "move_pane_focus", direction = "right" } },
  { kind = "command", command = { verb = "zoom_pane", zoom = "on" } },
  { kind = "command", command = { verb = "zoom_pane", zoom = "off" } },
]
```

Left and right alone are the recommended trade on an eight-key deck. Four arrows is half the
surface, and most herdr layouts are one split wide; add `up` and `down` if yours are genuinely
two-dimensional.

There is no `zoom_pane` toggle, and that is not an omission. Zoom is a per-tab flag herdr owns and
the deck only ever holds a second or two late, so a toggle would be a guess at which way it is
about to go. Two keys stating opposite ends are idempotent instead: press `on` twice and you are
still zoomed.

Pressing a direction at the edge of a layout does nothing and says nothing — see
[the default layout](../getting-started/default-layout.md#pane-control-on-large-decks).

### Closing a pane

```toml
{ kind = "close_pane" }
```

Its own binding rather than a `close_pane` command with an id written in, because a pane id in a
config file is a promise nobody can keep: herdr renumbers pane ids when a pane moves between
workspaces, so `w1:p2` written last month names whatever is second there now. This binding closes
whatever herdr reports as focused, and draws dimmed and refuses when herdr reports nothing.

Three things to know before binding it.

- **It only fires on a hold.** A tap flashes an alert saying so.
- **It cascades, and cannot tell you that it did.** The last pane of a tab closes the tab; the
  last tab closes the workspace. herdr answers the same bare `ok` for all three, so the key can
  report what it asked for and never what it did.
- **herdr may ask you to confirm.** If closing would take a whole worktree group with it and you
  have herdr's `confirm_close` turned on, herdr opens its own dialog and refuses the deck. The key
  alerts and says the question is waiting in herdr's window — the deck cannot answer it for you,
  and does not try to dismiss it either.

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
