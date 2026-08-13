# herdr-deck

Stream Deck control for [herdr](https://github.com/herdrdev/herdr).

When several coding agents run at once, the expensive moment is *noticing* that one has stopped
and is waiting on you. That signal is buried in a TUI you may not be looking at.

herdr-deck puts it on physical hardware. Every agent gets a key. A key turns red the moment its
agent needs a decision, and pressing it switches herdr to that agent **and** brings your terminal
to the front.

![The default Stream Deck + layout](./images/deck-preview.png)

That image is real output — `herdr-deckd --dry-run` renders it with no hardware and no herdr
running.

## What you get

- **Live status per agent**, in attention order. The agent that needs you most is always on the
  first key.
- **One press to focus.** herdr switches to that pane and your terminal window comes forward.
- **Dials** on a Stream Deck +: scrub agents, workspaces, tabs, or just the ones that need you,
  and press to jump there.
- **Workspace navigation** — toggle the deck between agents and workspaces.
- **macOS and Linux**, looking and behaving identically.

## Two things to know up front

**Elgato's Stream Deck application does not run on Linux.** It is macOS and Windows only, and
Linux Stream Deck hosts cannot load Elgato-format `.sdPlugin` packages. herdr-deck handles this
by putting all its logic in a daemon and giving it two thin frontends: the official plugin on
macOS, and a direct USB HID driver on Linux. Both draw pixels rendered by the same code, so the
deck looks the same on both. See [Architecture](./reference/architecture.md).

**herdr cannot raise your terminal window, so herdr-deck does it.** herdr's API switches its own
active pane; bringing the OS window forward is a separate job with a different answer on every
desktop. herdr-deck detects your environment and picks a backend at runtime. One case genuinely
cannot work — [GNOME on Wayland](./focus/gnome-wayland.md) blocks programmatic window activation
— and herdr-deck tells you so instead of quietly doing half the job.

## Where to start

- [Install](./getting-started/install.md)
- [What the default layout does](./getting-started/default-layout.md)
- [`blocked` and `done`](./concepts/attention.md) — the two states the whole thing is built around
