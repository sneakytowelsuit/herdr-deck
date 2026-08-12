# Frontend protocol

How `herdr-deckd` talks to a frontend. You only need this if you are writing one — for a Linux
host herdr-deck does not support, say.

Newline-delimited JSON over a unix socket at `$XDG_RUNTIME_DIR/herdr-deck.sock` (on macOS,
`~/Library/Application Support/herdr-deck/herdr-deck.sock`), mode `0600`.

Deliberately not a TCP port: nothing needs to reach this off-box, and filesystem permissions
already express exactly the access we want.

## The division of labour

The frontend is dumb on purpose. It reports its hardware, forwards input, and draws what it is
given. **Every** decision about what a key means, what it shows, and what pressing it does
happens in the daemon.

## Handshake

The first message must be `hello`.

```json
{"type":"hello","frontend":"streamdeck-macos","protocol":1,
 "device":{"model":"plus","model_name":"Stream Deck +","columns":4,"rows":2,
           "key_image_px":120,"dials":4,
           "touchstrip":{"width":800,"height":100}}}
```

Only `columns`, `rows` and `key_image_px` are required. `model` is a hint; report geometry and
the daemon lays out from that, which is why hardware it has never heard of still works.

Report the controls you can actually **drive**, not the ones the hardware has. If you only own
half the deck, say so, and the daemon lays out for half a deck.

The daemon replies:

```json
{"type":"ready","protocol":1,"keys":8,"dials":4,"device":"Stream Deck + — 4x2 keys (120px), 4 dials, touchstrip 800x100"}
```

`protocol` is checked. On a mismatch the daemon replies `protocol_mismatch` and closes — the
plugin and daemon install through different mechanisms, so version skew is real rather than
theoretical.

You may send `hello` again mid-connection; if the capabilities changed, the daemon rebuilds the
layout and repaints.

## Frontend → daemon

| Message | Fields |
|---|---|
| `hello` | `frontend`, `device`, `protocol` |
| `key_down` / `key_up` | `index` |
| `dial_rotate` | `dial`, `ticks` (signed) |
| `dial_down` / `dial_up` | `dial` |
| `touch_tap` | `dial` (nullable) |
| `refresh` | — ask for a full repaint |
| `ping` | — |

Keys are numbered in reading order: `index = row * columns + column`.

**Send `key_up` for every `key_down`.** The daemon times the gap: half a second or more is a long
press, which on an agent key acknowledges it instead of focusing it. A frontend that only reports
presses will leave agent keys apparently dead, because a key with two meanings cannot be resolved
until it is released. Keys with a single meaning still act on `key_down`, as they always have.

Send `refresh` after connecting and whenever a control appears with no image — the daemon only
sends what changed, so it will not otherwise repaint a key it believes is already correct.

## Daemon → frontend

| Message | Fields |
|---|---|
| `ready` | `protocol`, `keys`, `dials`, `device` |
| `set_key_image` | `index`, `png` (base64) |
| `set_dial_feedback` | `dial`, `title`, `value`, `png` (base64, optional) |
| `ok` | `index` — the action fully succeeded |
| `alert` | `index`, `message` — it did not |
| `protocol_mismatch` | `expected`, `got` |
| `pong` | — |

`set_dial_feedback` carries both text and an optional image because the two frontends need
different things: the macOS plugin renders the touchstrip through Elgato's own layout system and
uses only `title`/`value`, while the Linux driver writes the PNG straight to the LCD. The `png`
field is omitted when there is no touchstrip.

`alert` is how a *partial* success reaches the user — herdr focused but the window did not come
forward. Surface it visibly; otherwise someone presses a key and nothing appears to happen.

## Notes for implementers

- **Buffer partial reads.** A socket read boundary lands wherever the kernel decides. A message
  can arrive in pieces or several can arrive at once.
- **Skip lines you cannot parse** rather than dropping the connection.
- **Reconnect on your own.** The daemon restarts (upgrades, `service restart`) and the user
  should not have to do anything.
- **Re-send `hello` on reconnect.** A fresh daemon has no memory of you.
