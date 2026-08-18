# Control families

herdr-deck drives herdr's *structure* — where you are and how the panes, tabs, workspaces and
worktrees around you are arranged. Everything it can do is one of five families, and each family
is a [page](../getting-started/default-layout.md#pages).

This page is the whole surface, in one place: what each family does, what it deliberately does
not, and why.

## The boundary, first

**herdr-deck never interacts with the agents inside the panes.** No approve, no deny, no canned
replies, no interrupt, no keystrokes sent into a pane, ever. It shows you which agent needs you
and takes you there; the conversation is yours to have, at your keyboard, in your terminal.

That is a product boundary and not a to-do item. herdr's API has the methods — `pane.send_keys`,
`agent.prompt`, `agent.send_keys` — and herdr-deck does not call them and will not grow a setting
that does. A deck answering a question you have not read is the failure mode this project exists
to avoid.

The second boundary is smaller and follows from the hardware: **a deck has no keyboard.** Anything
that needs free text comes from a [named preset](configuration.md#presets-are-how-text-reaches-herdr)
in your config, or it is left out and the reason is written down.

## agents — the reason the deck is on the desk

| Control | Does |
|---|---|
| Any agent key, tapped | Focus that agent: herdr switches workspace, tab and pane, and the terminal window is raised. |
| Any agent key, held | Acknowledge it. herdr is **not** called and nothing moves on your screen. |
| The attention key | Go to the agent that needs you most, from any page, and bring the deck home. |

One call does the whole journey. herdr's `agent.focus` switches the workspace, switches the tab,
focuses the pane, follows zoom and dismisses whatever dialog herdr was in — so the deck sends one
call and never a chain of three, which would fight herdr's own logic and cost two extra round
trips.

Acknowledging is the deliberate opposite: it is a statement about what *you* have seen, held by
the deck, per connected frontend. See [`blocked` and `done`](../concepts/attention.md).

## spaces — herdr's workspaces

A list. Press one to go there, landing on whichever tab it had active.

There is **no key that closes a workspace**, and there will not be one until the daemon can
establish something it currently cannot. `workspace.close` has no confirmation, and if the target
is the source-repository member of a worktree group it closes *every* workspace in that group —
returning a bare `ok`, so the key could not even report what it had destroyed. The check a future
daemon would have to make is written down in
[the protocol notes](herdr-protocol.md#why-there-is-no-key-that-closes-a-workspace).

`workspace.rename` is out for the keyboard reason: free text with no clear form.

## trees — git worktrees

Only appears when the session is inside a repository that has worktrees. Every checkout gets a
key; pressing one opens it as a workspace and takes you there, window raise and all — it is the
one structural command that is a *journey* rather than a change to where you already are.

A checkout herdr already holds open is drawn filled and captioned `open`; one it does not is
drawn hollow. Pressing either does the same thing, because `worktree.open` is idempotent.

Worktrees are opened **by path**, not by branch: a detached checkout has no branch, and a list
where some rows worked would be worse than a shorter one.

Removing a worktree is destructive, is not in any derived layout, and is
[bound by hand](configuration.md#removing-a-worktree). It is **never forced** and there is no
setting that would force it — git's refusal to remove a dirty checkout is the confirmation this
hardware has no screen to show, and that refusal lands on the key rather than being swallowed.

## panes — moving around the tab you are in

Six keys: `left`, `right`, `up`, `down`, `zoom`, `unzoom`. Each is one press, one outcome,
nothing to type — which is what makes them worth a physical key at all.

Three things they deliberately do.

**They never raise the terminal window.** An agent key is half a journey until the window comes
forward; a pane key is something you press while sitting at the terminal it is about. Raising
anyway would spend a round trip on every press and, on a desktop that
[cannot raise windows](../focus/gnome-wayland.md), would make every one of these keys alert.

**They name a direction and never a pane.** herdr's `pane.focus_direction` will navigate to a
named pane first if you give it one, so a key labelled `left` would also teleport whoever pressed
it. It carries a direction and nothing else.

**Zoom states an end, never a toggle.** Zoom is a per-tab flag herdr owns and the deck only ever
holds a second or two late, so a toggle would be a guess at which way it is about to go. Two keys
stating opposite ends are idempotent instead: press `zoom` twice and you are still zoomed.

And one thing they deliberately do not: **reaching the edge of a layout is not an error.** herdr
answers "there is no pane that way" as a plain success, and so does the deck — no alert, no
flash. A thumb run along a row of arrows reaches an edge every time, and a key that cried wolf
for it would teach you to ignore the alerts that matter. The press is still recorded, as
`unchanged`, in the [command log](architecture.md).

## make — bringing something into being

Five keys, plus one per [named layout](configuration.md#layoutsname--arrangements-of-panes) in
your config:

| Key | Does |
|---|---|
| `split right` / `split down` | New shell beside or below the focused pane, and focuses it. |
| `new tab` | New tab in the workspace you are in. |
| `new space` | New workspace. |
| `new tree` | New git worktree, opened and focused. herdr invents the branch name. |
| one per named layout | Builds that arrangement of panes as a **new tab**. |

Splitting lives here rather than with the pane arrows because it does not move you around an
arrangement — it makes a new shell. That grouping also happens to leave the panes page at exactly
six keys, which is what a Stream Deck + has room for.

**A layout key only ever adds a tab.** herdr's `layout.apply` can also be aimed at an existing
tab, in which case it builds the replacement and then closes the one you named, killing every
process in it. herdr-deck's client offers no way to supply a tab id, so the destructive form
cannot be expressed rather than merely being avoided.

`new tree` waits on git, so on a large repository the key can take a few seconds to report back.
The rest of the deck keeps working while it does.

Closing a tab is destructive, is not in any derived layout, and is
[bound by hand](configuration.md#closing-a-tab).

## Destructive commands, wherever they come from

Three exist: close pane, close tab, remove worktree. All three behave identically.

- **Never in a derived layout.** You ask for them in your config or you do not have them.
- **A tap refuses out loud.** The key alerts with `hold to close`, rather than going quiet — a key
  that appears broken is worse than one that says no.
- **A hold is what issues the command**, the same half-second gesture the deck already uses for
  acknowledging an agent. There is one confirmation idiom on this hardware and this is it.
- **The key says so before it is pressed**, with `hold` in the corner where an agent key's wait
  marker sits.
- **A dial can only refuse.** An encoder has no gesture that means "I am sure", and inventing one
  would be worse than not offering it.

The guard is applied to the *action*, not to the bindings that happen to produce one today — so a
command added tomorrow is guarded the day it is added, and a hand-written config cannot opt out.

## What is recorded

Every command that actually reaches herdr leaves one line in the
[command log](architecture.md): when, which command, what it named, and what came of it. Nothing
else does — changing pages, scrubbing a dial and acknowledging an agent never touch herdr, so
there is nothing to record.

Targets are herdr's own ids, words from a closed set (`left`, `on`), or preset names — which is
why preset names are held to a safe spelling when the config loads. Two commands record no target
at all: opening and creating a worktree, whose only identifier is a path or a branch out of your
own repository. Those belong to you, not to the log.
