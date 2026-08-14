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

By default every key shows the Nth entry of whichever
[page](../getting-started/default-layout.md#pages) the deck is on, which is what makes herdr-deck
useful with no configuration. If you want specific keys pinned, replace the whole layout:

```toml
[layout]
keys = [
  # The Nth entry of the current page — the Nth agent when the deck is showing
  # agents, the Nth workspace when it is showing spaces, and so on.
  { kind = "dynamic", rank = 0 },
  { kind = "dynamic", rank = 1 },

  # The Nth entry of ONE page, whatever page the deck is on. This is how a deck
  # with keys to spare shows two families at once instead of paging between them.
  # Pages: agents | spaces | trees | panes | make
  { kind = "dynamic", rank = 0, page = "panes" },

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

  # Structure. Each of these is described under `Layouts, worktrees and presets`.
  { kind = "layout", preset = "dev" },
  { kind = "new_workspace", preset = "notes" },
  { kind = "new_tab" },
  { kind = "new_worktree" },
  { kind = "close_tab" },
  { kind = "remove_worktree" },

  # Fixed controls. Bind both of the first two on any deck you can page: they are
  # the way home and the way anywhere else.
  { kind = "attention" },      # to the agent that needs you, and back to the agents page
  { kind = "page_cycle" },     # agents / spaces / trees / panes / make
  { kind = "screen_prev" },    # within a page longer than the keys showing it
  { kind = "screen_next" },

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

Scrub targets: `agents`, `workspaces`, `tabs`, `attention`, `worktrees`. The first four are what
dials get by default, in that order; `worktrees` is there to be bound by hand.

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
| `create_worktree` | — | New git worktree, opened and focused. |
| `open_worktree` | `path` | Open that checkout as a workspace and go there. |

A hand-written layout shorter than the hardware is padded with empty keys, so you cannot
accidentally leave keys unaddressable.

Four of these were renamed when the mode toggle
[became a page cycle](../getting-started/default-layout.md#pages). The old spellings still load,
so an existing config keeps working:

| Old | New |
|---|---|
| `next_attention` | `attention` |
| `mode_toggle` | `page_cycle` |
| `page_prev` | `screen_prev` |
| `page_next` | `screen_next` |

The words moved because a *page* now means a family of controls, and the thing `page_prev` moved
through is a screen within one.

> **Leave yourself a way home.** `attention` is the only key that returns to the agents page, and
> a hand-written layout without one on a deck that can reach another page is a deck you can walk
> into a corner of. The derived layout always has one; yours has to say so.

> A command that could destroy work is never issued by a tap. It moves to a hold, the key wears
> `hold` on its face, and tapping it says so rather than going quiet. The guard applies to the
> command, not to the key, so a hand-written layout cannot opt out of it.

## Pane control

Every deck reaches pane control on the [panes page](control-families.md#panes--moving-around-the-tab-you-are-in),
and a deck with [24 keys or more](../getting-started/default-layout.md#what-a-big-deck-buys-you)
shows that page permanently on keys of its own. What follows is how to put pane keys somewhere
else — a fixed cluster on a small deck, say, so they are in the same place whichever page you are
looking at.

The tidy way is to pin the page:

```toml
[layout]
keys = [
  { kind = "dynamic", rank = 0 },
  { kind = "dynamic", rank = 1 },
  { kind = "dynamic", rank = 0, page = "panes" },   # left
  { kind = "dynamic", rank = 1, page = "panes" },   # right
  { kind = "dynamic", rank = 4, page = "panes" },   # zoom
  { kind = "dynamic", rank = 5, page = "panes" },   # unzoom
  { kind = "attention" },
  { kind = "page_cycle" },
]
```

Pinned keys never page, so each one shows exactly the entry you named. A page that is only *partly*
pinned this way stays in the cycle, so the entries you left out are still reachable.

The explicit way, which does not depend on the order of a page:

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
[control families](control-families.md#panes--moving-around-the-tab-you-are-in).

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

## Layouts, worktrees and presets

Six more bindings drive herdr's own structure — the workspaces, tabs, layouts and git worktrees
around your agents rather than the agents themselves.

| Binding | What it does |
|---|---|
| `{ kind = "layout", preset = "dev" }` | Builds a **new tab** from `[layouts.dev]`. |
| `{ kind = "new_workspace", preset = "notes" }` | New workspace from `[workspaces.notes]`. `preset` is optional. |
| `{ kind = "new_tab", preset = "logs" }` | New tab in the workspace you are in, from `[tabs.logs]`. `preset` is optional. |
| `{ kind = "new_worktree" }` | New git worktree, opened and focused. Takes nothing. |
| `{ kind = "close_tab" }` | **Destructive.** Closes the focused tab. Hold. |
| `{ kind = "remove_worktree" }` | **Destructive.** Gives the focused worktree back to git. Hold. |

A key naming a preset that does not exist is a **config error**, not a dead key. The deck says
which preset it wanted and lists the ones you have.

### Presets are how text reaches herdr

A Stream Deck has no keyboard, so nothing that needs typing can be a key. Presets are the way
round that: write the working directory, the label or the environment down once, give it a name,
and bind a key to the name.

```toml
[workspaces.notes]
label = "notes"
cwd = "/home/dev/notes"
env = { EDITOR = "hx" }

[tabs.logs]
label = "logs"
```

Both tables take the same three optional fields, and every one of them is optional — a
`new_workspace` key with no preset still works, because herdr picks the working directory from the
pane you are in.

Preset names must be lower-case letters, digits, `-` or `_`. That is not fussiness: a preset name
goes on a key face and into the [command log](architecture.md), and it is the only thing recorded
there that a person wrote.

This is also why `workspace.rename` and `tab.rename` have no keys at all. Both need free text with
no clear form, so the best a key could do is cycle through canned labels — see
[the protocol notes](herdr-protocol.md#structure-layouts-worktrees-workspaces-and-tabs).

### `[layouts.<name>]` — arrangements of panes

A layout is a tree. A node with `split` divides its space; a node without one is a pane.

```toml
[layouts.dev]
# What to call the tab this makes. Optional.
label = "dev"

# 70% editor on top, a test runner underneath.
[layouts.dev.root]
split = "down"      # "down" or "right" — herdr has no split-left or split-up
ratio = 70          # whole percent, 10 to 90, for the FIRST child

[layouts.dev.root.first]
# A bare pane: your shell, in the current directory.

[layouts.dev.root.second]
command = ["cargo", "watch", "-x", "test"]
cwd = "/home/dev/src/api"
label = "tests"
```

Pane nodes take `cwd`, `command` and `label`; split nodes take `split`, `ratio`, `first` and
`second`. Mixing the two on one node is an error, as is a `split` missing either child, or
children with no `split` saying which way they divide. herdr accepts at most 24 panes and 16
levels of nesting, and the deck checks both when the config loads rather than letting the key fail
when it is pressed.

`ratio` is refused outside 10–90 rather than clamped. herdr silently clamps it, which would leave
a config saying `95` and a deck quietly doing `90` forever.

**A layout key only ever adds a tab.** `layout.apply` can also be pointed at an existing tab, in
which case it builds the replacement and then closes the one you named, killing every process in
it. herdr-deck has no way to express that, and
[will not grow one](herdr-protocol.md#structure-layouts-worktrees-workspaces-and-tabs).

Layouts are the one preset that reaches a key on its own: each named layout becomes an entry on
the [make page](control-families.md#make--bringing-something-into-being), alphabetically, without
being asked. A layout nobody can press is a layout nobody meant to write down.

It costs no keys. Ten presets lengthen a page rather than swallowing a deck — a deck with room
shows as many as it has room for and keeps the make page in the cycle for the rest; a deck without
room reaches the whole page through the cycle anyway. Workspace and tab presets are bound by hand.

### Worktrees

If your session is inside a git repository, the page key grows a **trees** stop listing every
worktree of that repository. Pressing one opens it as a workspace and takes you there, window and
all, exactly as pressing an agent does. A checkout herdr already has open is drawn with a filled
circle and the word `open`; one it does not have open is drawn hollow. Pressing either does the
same thing, because `worktree.open` is idempotent.

Sessions with no worktrees never see the stop, so nobody pays a press for a page they do not use.
There is also a dial target:

```toml
dials = [{ kind = "scrub", target = "worktrees" }]
```

The listing makes herdr shell out to git, so it is refreshed on a slow timer and whenever the set
of workspaces changes — which is what opening, creating or removing a worktree always does. In
practice a worktree key reflects reality within a reconcile.

`{ kind = "new_worktree" }` is the only key on this deck that *starts* a piece of work rather than
navigating to one, and it takes no arguments at all: herdr generates the branch name, bases it on
`HEAD`, and puts the checkout in its configured worktree directory. It waits on git, so on a large
repository the key can take a few seconds to report — the rest of the deck keeps working meanwhile.

### Removing a worktree

```toml
{ kind = "remove_worktree" }
```

Gives the focused workspace's checkout back to git, and closes the workspace. Four things to know.

- **It only fires on a hold**, like every destructive key here.
- **It is never forced, and there is no setting that would force it.** Unforced, git refuses to
  remove a checkout holding uncommitted or untracked work, and that refusal appears on the key.
  That is the confirmation prompt — a real one, from the tool that actually knows.
- **Committed work always survives.** herdr never deletes the branch.
- **It draws dimmed and refuses when the focused workspace is not a linked worktree**, because
  herdr will not remove a repository's source checkout and the deck already knows which one that
  is.

### Closing a tab

```toml
{ kind = "close_tab" }
```

The same bargain as [`close_pane`](#closing-a-pane), one level up: hold to fire, cascades to the
whole workspace when it is the last tab, and cannot tell you afterwards which of the two happened.

**There is no key that closes a workspace**, and there will not be one. `workspace.close` has no
confirmation and does not necessarily close one workspace —
[the reasoning is written down](herdr-protocol.md#why-there-is-no-key-that-closes-a-workspace).

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
