# `blocked` and `done`

herdr classifies every agent into one of five states:

```
idle | working | blocked | done | unknown
```

Two of them are why herdr-deck exists.

## `blocked` — needs you now

> The agent needs input, approval, or a decision.

This is the state the whole product is built around. A blocked agent is doing nothing until you
look at it, and every second it stays unnoticed is wasted. It gets the loudest treatment on the
deck: a full red key, a `!` glyph, and first place in the ordering.

## `done` — finished, unreviewed

> The agent finished and you have not looked at it yet.

`done` is not a separate lifecycle state — it is `idle` *plus* "unseen". Focusing the pane marks
it seen and it becomes `idle`. So pressing a key on a `done` agent both takes you there and
clears it from the attention queue.

Reading state through the herdr CLI does **not** mark it seen; only focusing does.

That coupling is exactly right for `blocked`: an agent that needs a decision needs *you*, at the
keyboard, now. It is friction for `done`. Clearing three finished agents by pressing their keys
would yank your terminal window forward three times, for three agents you have already decided
you do not need to look at yet.

## Long press to acknowledge

**Hold an agent's key for half a second and it leaves the attention queue without being focused.**
herdr is not called and no window moves. The agent's key still shows it — you dismissed the
alarm, not the agent — but it stops counting towards the attention key, drops below everything
still asking for you, and its tile goes calm with a `×` in place of its status glyph.

A short press is unchanged: it focuses, exactly as it always did. Only keys that show an agent
have a second action, and the one other kind of key that does — anything
[destructive](../reference/control-families.md#destructive-commands-wherever-they-come-from) —
has it the other way round. Holding a page key or a paging key does what pressing it does.

### An acknowledgement expires the moment the agent moves

This is the part that matters. An acknowledgement is recorded against the agent **and the exact
state it was in** — herdr's `state_change_seq`. The instant that sequence changes, the
acknowledgement stops describing anything and the agent is back in the queue on its own.

So an agent you dismissed while `done`, which later blocks waiting for an approval, reappears
immediately and loudly. Dismissing something is never a way to stop hearing from it again.

If herdr reports an agent with no `state_change_seq` at all, the deck **refuses** to acknowledge
it and flashes an alert on that key, because nothing would ever expire the acknowledgement.

Acknowledgement lives with the connected frontend, not with the daemon. Two decks attached to one
herdr each keep their own: dismissing an alert on the deck in front of you does not silence the
one in the next room, and reconnecting a deck starts it from a clean queue.

## How long it has been asking

A key that has been red for eight minutes looks exactly like one that turned red a moment ago,
and they are not the same problem. So an agent in an attention state also carries a small
duration marker in the corner opposite its status glyph, and its status bar thickens as the wait
grows:

| Marker | Meaning              | Bar      |
| ------ | -------------------- | -------- |
| `<1m`  | under a minute       | normal   |
| `1m+`  | one to five minutes  | thicker  |
| `5m+`  | over five minutes    | thickest |

Buckets, not a live counter, and deliberately so. The daemon only repaints keys whose contents
changed; a ticking clock would change every waiting key every second and repaint the deck
forever. Three buckets cost at most two repaints per agent per incident.

Only `blocked` and `done` are timed — a `working` agent is not waiting for anyone — and a
[dismissed](#long-press-to-acknowledge) agent drops its marker, because you have already said you
saw it.

### The clock is the deck's own

herdr records no timestamps at all. `state_change_seq` is a monotonic counter, not a time, so
nothing herdr reports can answer "how long". The daemon therefore notes when it first saw each
agent enter its current attention state, keyed on the agent **and** that sequence — the same
trick acknowledgement uses, and for the same reason. An agent that unblocks and blocks again is
asking a new question, and its clock starts again with it.

Two consequences worth knowing:

- The clock starts when **the daemon** first saw the state, not when the agent entered it. Start
  herdr-deck next to an agent that has already been blocked for an hour and it will read `<1m`
  until it crosses the next boundary. There is no way to do better; nothing recorded the truth.
- herdr briefly going away does not reset it. An agent still blocked on the same sequence when
  herdr comes back has been waiting the whole time.

If herdr reports an agent with no `state_change_seq`, it gets no marker rather than a guess.

## The others

- **`working`** — running. Interesting, but not actionable.
- **`idle`** — finished or waiting, and already seen.
- **`unknown`** — herdr could not confidently classify it.

## An important caveat about detection

herdr only marks an agent `blocked` when a live snapshot of the terminal matches a known
approval, question, or permission UI. If no rule matches for a recognised agent, it falls back to
`idle`.

**herdr-deck can only ever be as accurate as herdr's detection.** If an agent sits at a prompt
and its key stays dim, the deck is faithfully reporting what herdr believes. Diagnose it at the
source:

```sh
herdr agent explain <target>
```

That prints the final state, which detection manifest was used, which rule matched, and why a
fallback was taken. herdr auto-updates its manifests, so unusual prompts tend to get recognised
over time; you can also override detection per agent in
`~/.config/herdr/agent-detection/<agent>.toml`.

This is also why the deck surfaces `done` as prominently as it does. An agent that finished
without ever being detected as blocked still lands in your attention queue.

## Rollup

A blocked agent makes its pane, its tab, and its workspace look blocked. So in workspace mode a
red workspace key means "something in here needs you" — press it to go there.
