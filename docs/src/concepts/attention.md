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
clears it from the attention queue, which is exactly the behaviour you want.

Reading state through the herdr CLI does **not** mark it seen; only focusing does.

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
