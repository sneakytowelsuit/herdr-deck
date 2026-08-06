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
