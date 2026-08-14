# Architecture

## The constraint that shapes everything

Elgato's Stream Deck application runs on **macOS and Windows only**. It has never run on Linux,
and Linux Stream Deck hosts cannot load Elgato-format `.sdPlugin` packages.

So a single plugin cannot cover both of herdr-deck's target platforms. The answer is one shared
core with two thin frontends.

```text
                    ┌──────────────────────────────┐
                    │        herdr (server)        │
                    │  ~/.config/herdr/herdr.sock  │
                    └───────────▲──────────────────┘
                       NDJSON   │  session.snapshot  (bootstrap + reconcile)
                                │  events.subscribe  (persistent push)
                                │  agent.focus / workspace.focus
                    ┌───────────┴──────────────────┐
                    │        herdr-deckd           │  ← all logic lives here
                    │  state cache · layout engine │
                    │  tile renderer (SVG → PNG)   │
                    │  focus engine + window raise │
                    └──────┬────────────────┬──────┘
              NDJSON over  │                │  USB HID
              unix socket  │                │  (Linux only)
                    ┌──────▼───────┐   ┌────▼──────────────┐
                    │  .sdPlugin   │   │ hidapi driver     │
                    │  (TS, macOS) │   │ (in-daemon, Linux)│
                    └──────┬───────┘   └────┬──────────────┘
                  Elgato app WS             │
                    ┌──────▼───────┐   ┌────▼──────────────┐
                    │  Stream Deck │   │   Stream Deck     │
                    └──────────────┘   └───────────────────┘
```

## Why the daemon renders the images

`herdr-deckd` rasterises each tile to a PNG at the device's native key size and ships it base64
to whichever frontend is connected. The macOS plugin calls `setImage`; the Linux driver writes it
to HID.

If each frontend drew its own tiles they would drift — different fonts, different metrics,
different rounding — and every visual fix would need doing twice. Rendering centrally makes "it
looks the same on both machines" true by construction rather than by discipline. It also makes
the macOS plugin a few hundred lines of transport, which is exactly where you want your
platform-specific code to be.

The font is vendored rather than taken from the system, so a CI runner and a laptop produce
byte-identical output. That is what makes the golden-image tests meaningful.

## Why the Linux driver is a client, not a special case

On Linux the daemon drives the deck itself — but it does so by connecting to its **own frontend
socket** as an ordinary client. It could have called into the layout engine directly. Going
through the socket means there is exactly one protocol, one set of semantics, and one code path
where a key press turns into an action. A bug can be reproduced on either platform.

## Snapshot is truth, events are a doorbell

The obvious way to track herdr is to apply events to a local cache. herdr-deck deliberately does
not. Events decide only *when* to re-read `session.snapshot`; they never decide *what* the state
is.

That costs one extra round trip per burst of activity, and buys immunity to every dropped-event,
out-of-order, and missing-field bug an event-sourced cache is exposed to. A status display that
is 200ms late is fine. One that is subtly wrong is not.

Three details make it hold up:

- The subscription opens **before** the first snapshot, so no change can slip through the gap.
- Event bursts are coalesced — one user action in herdr emits several events and costs one
  snapshot.
- A slow timer reconciles regardless, so a dropped event costs latency rather than a frozen deck.

## Repainting only what changed

Each tile is hashed. A reconcile that produces identical tiles sends nothing at all, so the
safety-net timer does not repaint the deck twice a second forever.

## One command, carried end to end

Everything the deck asks herdr to *do* is a `DeckCommand`. A control decides which one it means,
the daemon performs it, the audit log records it — and all three handle the same value rather
than translating it into a shape of their own on the way past.

```text
   key or dial            daemon                    socket
  ┌───────────┐      ┌──────────────┐        ┌──────────────────┐
  │ SlotAction├─────►│ PendingAction├───────►│ focus engine     │
  │ ::Command │      │ { command,   │        │ one match, one   │
  └───────────┘      │   key }      │        │ call per command │
                     └──────┬───────┘        └──────────────────┘
                            │
                            ▼
                     ┌──────────────┐
                     │  audit log   │
                     └──────────────┘
```

Adding a command means a variant and an arm where commands are performed. It deliberately does
**not** mean a new match arm in every layer it passes through, which is what a control surface
with fifteen commands would otherwise cost.

Two things are kept out of that vocabulary on purpose:

- **herdr's method names.** A command names an intent; `herdr-deck-herdr` decides what that
  becomes on the wire.
- **Agent focus before it is resolved.** The deck binds agents by their stable `terminal_id` and
  herdr focuses *panes*, so agent focus stays a `SlotAction` until the daemon resolves it against
  live state at the moment the key is released. An unresolved target therefore cannot reach the
  socket, and an agent that vanished between paint and press is reported rather than guessed at.
  Closing a pane is resolved the same way and for the same reason: pane ids are renumbered when a
  pane moves workspaces, so the only id safe to close is the one herdr just gave us.

## Not every command is a journey

A focus is two steps — herdr's focus, then the OS window — and is only half done until the window
comes forward. Pane control is one step. Moving one pane left, zooming, splitting and closing all
rearrange the terminal you are already sitting at, so they stop after herdr and the key reports a
plain success.

That line is drawn once, on the command, so a key cannot disagree with it. Getting it wrong in
either direction costs something real: raising a window on every arrow press would spend a round
trip nobody asked for and would make every one of those keys alert on a desktop that cannot raise
windows at all, while *not* raising for an agent focus would leave you looking at the wrong window
and wondering why the key did nothing.

## Three ways a command can not-happen

herdr distinguishes "I refused" from "I understood, and there was nothing to do", and the deck
carries that distinction all the way to the key.

| Verdict | On the key | When |
|---|---|---|
| `complete` | Ok | It happened, window and all. |
| `settled` | Ok | herdr did it and there was nothing further — nothing attached to raise, or a command that never wanted the window. |
| `unchanged` | Ok | herdr understood and found nothing to change: the edge of a layout, a tab with one pane, a zoom already on. |
| `partial` | Alert | herdr did it; the window should have come forward and did not. |
| `failed` | Alert | herdr refused, so nothing happened at all. |

Only the last two are alerts. The middle one is why: a thumb run along a row of direction keys
reaches an edge every single time, and a deck that flashed for it would teach its owner to stop
reading the flashes — which are the only thing this hardware has to say something is genuinely
wrong. The log keeps all five apart, so "I pressed left four times and only moved twice" is still
a question with an answer.

## Reads are polled, writes are commands

The watcher re-reads `session.snapshot` on a timer whether or not anyone touches the deck, so a
read is never something a user did. A command always is. That split is why the audit log records
every command and no read: dismissing an agent from the attention queue, changing page or turning
a dial rearranges the deck's own view of herdr and leaves no line, because from herdr's side
nothing happened.

## The audit log

One JSONL line per command actually issued, in `commands.jsonl` under the daemon's state
directory — timestamp, command, target id, outcome. It rotates once at a quarter of a megabyte,
so it is bounded at twice that and never needs thinking about.

It is written on a task of its own behind a bounded channel: a slow or full disk costs audit
lines, never key latency, and a dropped line says so in the daemon's log rather than leaving an
unmarked hole. A line carries ids and a closed vocabulary of outcomes, never a label, window
title or working directory — a trail that quoted those would be a file to guard like a secret
rather than one to read like a log.

## One confirmation idiom

A long press already means "I am sure" on this deck: it is how an agent is dismissed from the
attention queue without focusing it. Anything that could destroy work therefore fires from a hold
and never from a tap, the key wears `hold` on its face, and tapping it says so rather than going
quiet. Closing a pane is the one command this applies to today, and it is never in a layout the
deck derived for you — you have to bind it. Guarding is a property of the command, not of the key, so it cannot be bypassed by binding
one by hand — and it applies the day a new command is added rather than the day someone remembers.

A dial has no hold, since the daemon ignores an encoder's release. A destructive command on a
dial is refused outright, which is the honest answer: there is no gesture there that means "I am
sure".

## The crates

| Crate | Responsibility |
|---|---|
| `herdr-deck-herdr` | Every fact about herdr's wire protocol, and nothing else. |
| `herdr-deck-core` | State, attention ordering, capabilities, layout, rendering, config, frontend protocol. |
| `herdr-deck-focus` | herdr focus plus OS window raising. |
| `herdr-deck-hid` | The Linux USB HID frontend. |
| `herdr-deckd` | The daemon: wires it together and serves frontends. |
| `herdr-deck-cli` | `herdr-deck`: doctor, status, layout, install, service, icons. |

`herdr-deck-herdr` exists as its own crate for one reason: when herdr changes its protocol,
exactly one crate needs to move. Nothing above it constructs a herdr method name or matches on a
herdr JSON field.

## Testing without hardware

The whole stack is testable on a machine with no deck and no herdr:

- `herdr-deck-herdr` ships an in-process **fake herdr server** behind its `mock` feature, which
  reproduces the two behaviours that actually bite: RPC connections closing after one response,
  and subscriptions staying open.
- The focus engine takes a command runner, so backend behaviour is tested with no desktop.
- The session is a pure state machine — messages in, messages out — so key semantics need no
  socket.
- `--dry-run` renders every tile to disk.
