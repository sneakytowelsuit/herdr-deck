# What the default layout does

There is nothing to configure. herdr-deck reads the geometry of whatever deck is attached and
lays itself out to fit.

![The default layout](../images/deck-preview.png)

## Pages

A deck has eight keys and herdr-deck drives about fifteen commands, so the keys show one **page**
at a time and a key walks between them. There are five:

| Page | What its keys are |
|---|---|
| **agents** | Every agent, in attention order. The default, and where the deck starts. |
| **spaces** | herdr's workspaces. Press one to go there. |
| **trees** | The git worktrees of the repository you are in — only when there are some. |
| **panes** | Move between the panes of the tab you are in, and zoom one to fill it. |
| **make** | Split a pane, make a tab, a workspace, a worktree, or a [named layout](../reference/configuration.md#layouts-workspaces-and-tabs). |

The first three are **lists**: what is on them changes as agents come and go. The last two are
**commands**: the same keys in the same places every time you arrive, which is what lets them be
pressed by feel. See [control families](../reference/control-families.md) for what each one can
and cannot do.

## The two keys that are on every page

**The attention key** — the one with the number on it — is the way home. Press it from anywhere
and two things happen: the deck comes back to the agents page, and if anything is asking for you,
herdr takes you to it. One press, from any page, to the agent that needs you. That is the promise
the whole deck is built around, and no amount of control surface is allowed to cost it a second
press.

It also means the count is visible from every page. Wander off to the panes page and something
blocks, and the key goes red and says so where you can see it.

**The page key** says which page you are on, in the size it is meant to be read at, with the page
the next press goes to underneath it in smaller type. You never have to count presses to work out
where you are. When a page is longer than the keys showing it, this key also carries `2/3` in its
top-left corner.

The cycle skips what it has nothing to show. A session with no worktrees never stops at **trees**,
so nobody pays a press for a page they do not use.

## Stream Deck + (8 keys, 4 dials, touchstrip)

| Control | What it does |
|---|---|
| Keys 1–6 | The current page. On the agents page, the first six agents in attention order. |
| Key 7 | Attention / home. Press to go to the agent that needs you most, and come back to the agents page. Dark when nothing does. |
| Key 8 | The page key. |
| Dial 1 | Rotate: scrub all agents. Press: focus. |
| Dial 2 | Rotate: cycle workspaces. Press: focus. |
| Dial 3 | Rotate: cycle tabs. Press: focus. |
| Dial 4 | Rotate: scrub **only** agents that need you. Press: focus. |

The touchstrip above each dial shows what that dial is pointing at, coloured by its status.
Tapping the strip does the same as pressing the dial under it.

The dials scrub *lists*, not pages — so they keep working exactly as they did, whichever page the
keys happen to be showing.

## Reading an agent key

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

An agent that is `blocked` or `done` also shows how long it has been asking, in the corner
opposite its glyph — `<1m`, `1m+` or `5m+` — with a status bar that thickens as the wait grows.
Three buckets rather than a running clock, for good reasons; see
[attention](../concepts/attention.md#how-long-it-has-been-asking).

## Pressing and holding

Every agent key has two actions. A press focuses the agent, taking you there and raising the
terminal window. **Holding it for half a second acknowledges it instead**: it leaves the attention
queue, herdr is never called, and nothing moves on your screen. That is how you clear a row of
finished agents without your terminal jumping to the front once per key.

Command keys act the instant you press them, with one exception: **anything that could destroy
work only fires on a hold**, and its key says `hold` in the corner before you ever touch it.
Nothing in the derived layout is destructive — you have to ask for those in your config — but the
guard is on the action rather than on the key, so it applies wherever one comes from.

## When a page is longer than the keys

A deck with paging keys uses them. A deck without — a Stream Deck +, whose dials scrub lists but
know nothing about a page of commands — turns its last key into `more 1/2` and pages with that
instead. It only does this on a page that actually overruns.

What it never does is show six of nine things and go quiet about the other three.

## Other hardware

The layout adapts by *capability*, not by model name:

| Hardware | What changes |
|---|---|
| **XL** (32 keys) | 16 agent keys across the top two rows; the bottom two rows are the **panes** and **make** pages, permanently, so the cycle is just the three lists. Paging keys, because there are no dials. |
| **MK.2** (15 keys) | 11 keys for the current page, plus paging, attention and the page key. |
| **Mini** (6 keys) | 4 keys for the current page, attention, page key. Every page is still reachable. |
| **Neo** (8 keys) | 4 keys for the current page, paging, attention, page key. |
| **Pedal** (no screens) | Nothing is drawn. Pedal 1 goes to the agent that needs you; 2 and 3 step through the attention queue. |
| **Anything newer** | Laid out from its reported geometry. Unknown hardware still works. |

See it for yourself without owning the hardware:

```sh
herdr-deck layout --model xl
herdr-deckd --dry-run /tmp/tiles --dry-run-model xl
```

## What a big deck buys you

Past **24 keys**, the deck stops paging between control families and simply shows them. On an XL
the bottom two rows are the panes page and the make page, always there, never a press away — and
because those pages are already on the deck, the page key skips them. The cycle becomes agents →
spaces → trees, which is the only thing left that a page can tell you that a key cannot.

That rule has a safety catch. A page is only dropped from the cycle when **all** of it fits. Name
four layouts in your config and the make page grows past the six keys the XL gave it, so it stays
in the cycle and the ones that did not fit are still reachable.

Below 24 keys nothing is pinned: a key spent on a pane arrow is a key taken off an agent, and the
agents are why the deck is on the desk.

## Worktrees, when you have them

If your session is inside a git repository with worktrees, the page key grows a **trees** stop.
Every checkout of that repository gets a key, and pressing one opens it as a workspace and takes
you there — the same press, and the same window raise, as pressing an agent. A checkout herdr
already has open is drawn with a filled circle and the word `open`; one it does not is drawn
hollow. Either way the press does the same thing.

Sessions with no worktrees never see the stop. Nothing about this needs configuring.

## The ordering rule

Agents sort by status band first — `blocked`, then `done`, then `working`, then `idle`, then
`unknown` — and within a band, the most recently changed comes first.

So when three agents are blocked, the one that *just* blocked is on key 1. Ties break on a stable
id, so keys never shuffle between repaints.
