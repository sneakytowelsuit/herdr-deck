# herdr-deck

Stream Deck control for [herdr](https://github.com/herdrdev/herdr) — the runtime your coding
agents live on.

When several agents run at once, the expensive moment is *noticing* that one has stopped and is
waiting on you. That signal is buried in a TUI you may not be looking at.

herdr-deck puts it on physical hardware. Every agent gets a key. A key turns red the moment its
agent needs a decision, and pressing it switches herdr to that agent **and** brings your terminal
to the front.

It is also a control surface for herdr itself: the same eight keys drive panes, tabs, workspaces,
layouts and git worktrees, on **pages** you walk with one key — and the agent that needs you is
one press away from every one of them.

![The default Stream Deck + layout](docs/src/images/deck-preview.png)

*Real output from `herdr-deckd --dry-run`, which renders the deck with no hardware and no herdr
running.*

## What you get

- **Live status per agent**, in attention order — the agent that needs you most is always first.
- **One press to focus**: herdr switches pane, and your terminal window comes forward.
- **One press home**, from any page: to the agent that needs you, and back to the agent list.
- **Pane control** — move between panes, zoom one to fill its tab, split a new shell.
- **Structure** — new tabs, workspaces and git worktrees, and named pane layouts from your config.
- **Dials** on a Stream Deck +: scrub agents, workspaces, tabs, or only the ones that need you.
- **Adapts to the hardware**: a 32-key XL shows whole control families at once; a 6-key Mini
  reaches all of them by paging. Both keep every key they can for agents.
- **macOS and Linux**, looking and behaving identically.

What it deliberately does **not** do: interact with the agents inside the panes. No approve, no
deny, no canned replies, no keystrokes sent into a terminal. It shows you which agent needs you
and takes you there; the conversation is yours to have. See
[control families](https://sneakytowelsuit.github.io/herdr-deck/reference/control-families.html).

## Install

```sh
herdr plugin install sneakytowelsuit/herdr-deck --yes
herdr plugin action invoke install --plugin sneakytowelsuit.herdr-deck
herdr-deck doctor
```

On macOS you also install the Stream Deck plugin; on Linux you add a udev rule. Both are covered
in the [install guide](https://sneakytowelsuit.github.io/herdr-deck/getting-started/install.html),
and `herdr-deck doctor` tells you if either is missing.

## Two things worth knowing

**Elgato's Stream Deck app does not run on Linux.** It is macOS and Windows only, and Linux hosts
cannot load `.sdPlugin` packages. herdr-deck puts all its logic in a daemon and gives it two thin
frontends — the official plugin on macOS, a direct USB HID driver on Linux — both drawing pixels
rendered by the same code.

**herdr cannot raise your terminal window, so herdr-deck does.** herdr's API switches its own
active pane; bringing the OS window forward is a separate job with a different answer on every
desktop. herdr-deck detects yours at runtime. One case genuinely cannot work — GNOME on Wayland
blocks programmatic window activation — and herdr-deck
[says so](https://sneakytowelsuit.github.io/herdr-deck/focus/gnome-wayland.html) rather than
quietly doing half the job.

## Documentation

**<https://sneakytowelsuit.github.io/herdr-deck/>**

- [What the default layout does](https://sneakytowelsuit.github.io/herdr-deck/getting-started/default-layout.html)
- [`blocked` and `done`](https://sneakytowelsuit.github.io/herdr-deck/concepts/attention.html) — the two states this is built around
- [Control families](https://sneakytowelsuit.github.io/herdr-deck/reference/control-families.html) — everything the deck can ask herdr for, and what it will not
- [Configuration](https://sneakytowelsuit.github.io/herdr-deck/reference/configuration.html)
- [Architecture](https://sneakytowelsuit.github.io/herdr-deck/reference/architecture.html)
- [Troubleshooting](https://sneakytowelsuit.github.io/herdr-deck/help/troubleshooting.html)

## Development

```sh
cargo test                                  # no herdr and no hardware needed
cd plugin && npm install && npm test
```

Linux builds need `libudev-dev` and `pkg-config`. See
[Contributing](https://sneakytowelsuit.github.io/herdr-deck/help/contributing.html).

## Status

Early. The full stack is built and covered by tests that run without a deck or a herdr, but it
has not yet been exercised against real hardware end to end. Bug reports very welcome.

## License

MIT. Bundled DejaVu fonts are under the Bitstream Vera license — see
[`assets/fonts/LICENSE-DejaVu.txt`](assets/fonts/LICENSE-DejaVu.txt).
