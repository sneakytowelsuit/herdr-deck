# What the default layout does

There is nothing to configure. herdr-deck reads the geometry of whatever deck is attached and
lays itself out to fit.

## Stream Deck + (8 keys, 4 dials, touchstrip)

| Control | What it does |
|---|---|
| Keys 1–6 | The first six agents **in attention order**. Press to focus, hold to acknowledge. |
| Key 7 | *Next attention* — jumps to the agent that needs you most. Dark when nothing does. |
| Key 8 | Toggles the deck between agents and workspaces. |
| Dial 1 | Rotate: scrub all agents. Press: focus. |
| Dial 2 | Rotate: cycle workspaces. Press: focus. |
| Dial 3 | Rotate: cycle tabs. Press: focus. |
| Dial 4 | Rotate: scrub **only** agents that need you. Press: focus. |

The touchstrip above each dial shows what that dial is pointing at, coloured by its status.
Tapping the strip does the same as pressing the dial under it.

## Reading a key

![The default layout](../images/deck-preview.png)

Each agent key shows its name, the workspace it lives in, and a status colour with a matching
glyph:

| Status | Colour | Glyph | Meaning |
|---|---|---|---|
| `blocked` | red | `!` | Needs input, approval, or a decision — **now**. |
| `done` | amber | `✓` | Finished, and you have not looked at it yet. |
| `working` | dark, blue accent | `▶` | Running. |
| `idle` | dim | `·` | Finished or waiting, and already seen. |
| `unknown` | dim | `?` | herdr could not classify it. |
| *acknowledged* | dim | `×` | You dismissed it with a long press; see [attention](../concepts/attention.md). |

Status is never carried by colour alone — every state has its own glyph too.

A focused agent gets a bright ring around its key, so you can see where herdr currently is.

## Pressing and holding

Every agent key has two actions. A press focuses the agent, taking you there and raising the
terminal window. **Holding it for half a second acknowledges it instead**: it leaves the attention
queue, herdr is never called, and nothing moves on your screen. That is how you clear a row of
finished agents without your terminal jumping to the front once per key.

Nothing else on the deck has a second action, so every other key still acts the moment you press
it.

## Other hardware

The layout adapts by *capability*, not by model name:

| Hardware | What changes |
|---|---|
| **XL** (32 keys) | 28 agent keys; paging keys appear because there are no dials to scrub with. |
| **MK.2** (15 keys) | 11 agent keys plus paging. |
| **Mini** (6 keys) | 4 agent keys, next-attention, and the mode toggle. |
| **Neo** (8 keys) | Same as Mini's approach, with 6 agent keys. |
| **Pedal** (no screens) | Nothing is drawn. Pedal 1 jumps to the agent that needs you; 2 and 3 step through the attention queue. |
| **Anything newer** | Laid out from its reported geometry. Unknown hardware still works. |

See it for yourself without owning the hardware:

```sh
herdr-deck layout --model xl
herdr-deckd --dry-run /tmp/tiles --dry-run-model xl
```

## The ordering rule

Agents sort by status band first — `blocked`, then `done`, then `working`, then `idle`, then
`unknown` — and within a band, the most recently changed comes first.

So when three agents are blocked, the one that *just* blocked is on key 1. Ties break on a stable
id, so keys never shuffle between repaints.
