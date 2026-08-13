# Hardware and how layouts adapt

herdr-deck never asks "is this a Stream Deck +". It asks:

- how many keys, in what grid?
- how large is a key image?
- are there dials, and how many?
- is there a touchstrip?

Everything else follows from the answers. That is why hardware released after this code was
written still gets a working layout.

## The degradation ladder

| Situation | What happens |
|---|---|
| Dials present | Scrubbing lives on the dials; every key shows an agent. |
| No dials, many keys | No scrubbing needed — more agents are visible at once. |
| No dials, few keys | Paging keys appear so every agent stays reachable. |
| No displays at all (Pedal) | Nothing is drawn; the keys still act. |

Fixed controls are also never allowed to crowd out the agents they navigate: on a small deck,
herdr-deck drops paging before it drops agent slots.

## Supported models

| Model | Keys | Dials | Notes |
|---|---|---|---|
| Stream Deck + | 8 (4×2) | 4 | Touchstrip, 120px keys. The best fit. |
| Stream Deck XL | 32 (8×4) | — | 96px keys. |
| Stream Deck / MK.2 | 15 (5×3) | — | 72px keys. |
| Stream Deck Neo | 8 (4×2) | — | 96px keys. |
| Stream Deck Mini | 6 (3×2) | — | 80px keys. |
| Stream Deck Pedal | 3 | — | No screens; actions only. |
| Anything else | reported | reported | Laid out from geometry. |

## Hot-plug

Changing decks needs no reinstall and no config edit. The frontend re-reports its geometry, the
daemon rebuilds the layout, and the deck repaints.

## Which controls herdr-deck actually drives

On macOS, herdr-deck only uses the keys and dials you have placed its actions on. If you give it
six keys of a Stream Deck +, it lays out for six keys. If its dials belong to another plugin, it
reports no dials and never renders touchstrip feedback that nothing would draw.

On Linux, the daemon owns the whole device.

## Previewing a layout

```sh
# What each control would do
herdr-deck layout --model xl

# The actual images, rendered to a directory
herdr-deckd --dry-run /tmp/tiles --dry-run-model xl
```

Both work with no hardware attached and no herdr running.
