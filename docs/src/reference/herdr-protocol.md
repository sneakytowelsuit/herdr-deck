# herdr protocol notes

What herdr-deck relies on, and the sharp edges found along the way. Written against **herdr
0.8.0, socket protocol 19**.

All of this lives in one crate, `herdr-deck-herdr`, so a herdr protocol change is a one-crate
fix.

## Transport

Newline-delimited JSON over a unix socket. Resolution order, matching herdr's own:

1. `--session <name>` → `~/.config/herdr/sessions/<name>/herdr.sock`
2. `HERDR_SOCKET_PATH`
3. `HERDR_SESSION=<name>`
4. `~/.config/herdr/herdr.sock`

There is also a `herdr-client.sock` alongside these. That is herdr's *internal* client protocol
between the TUI and the server — not a public API, and not used here.

## RPC is one-shot

**herdr closes the connection after a single response.** Every call opens a fresh connection,
writes one line, reads one line, and closes. This is the protocol, not a workaround, and code
that tries to pool connections will hang on its second request.

The only methods that hold a connection open are `events.subscribe` and `pane.graphics.stream`.

## Methods used

| Method | Why |
|---|---|
| `session.snapshot` | The source of truth for all state. |
| `events.subscribe` | The doorbell that says when to re-read it. |
| `agent.focus` | Switch to an agent. Also marks the tab seen. |
| `workspace.focus` | Switch workspace. |
| `tab.focus` | Switch to one tab, for the tab dial. |
| `client.window_title.set` | Stamp a marker so the exact terminal window can be found. |
| `ping` | Liveness. |
| `notification.show` | Optional toast; its `reason` doubles as "is a TUI attached". |
| `pane.focus_direction` | Move one pane left/right/up/down. |
| `pane.zoom` | Fill a tab with its focused pane, or put it back. |
| `pane.split` | New shell beside or below the focused one. |
| `pane.close` | Close a pane. |
| `worktree.list` | The checkouts of the repository you are in, for the worktree page. |
| `worktree.open` | Open a checkout as a workspace and go there. |
| `worktree.create` | Make a new worktree. herdr invents the branch name. |
| `worktree.remove` | Give a checkout back to git. Never forced. |
| `workspace.create` | New workspace, from a named preset. |
| `tab.create` | New tab in the workspace you are in, from a named preset. |
| `tab.close` | Close a tab. |
| `layout.apply` | Build a new tab from a named arrangement of panes. Never with a `tab_id`. |

## Pane commands

Four sharp edges, all found in herdr's source rather than the hard way.

- **Omit `pane_id`.** Both `pane.focus_direction` and `pane.zoom` accept one, and both *navigate
  to that pane first* when given one — switching workspace and tab on the way. A key labelled
  "left" that also teleported you would be a surprise, so the deck sends neither. Without an id
  they act on whatever herdr currently has focused, which is also fresher than anything the deck
  knows.
- **`pane.zoom` takes `on`/`off`, never `toggle`.** Zoom is a per-tab boolean; herdr's snapshot
  carries it, but the deck's copy is always a reconcile behind. Stating the end state is
  idempotent and self-correcting; `already_zoomed` comes back as a success, not an error.
- **`pane.focus_direction` at an edge is a success.** It answers `changed: false` with reason
  `no_neighbor` — note herdr's American spelling — and so are `single_pane` and
  `already_unzoomed`. None of them are errors and none of them may reach a key as one.
- **`pane.split` has only `right` and `down`.** There is no split-left or split-up to offer. It
  also un-zooms the tab as a side effect, and the deck asks for `focus: true` so the new shell is
  the one you are in.

`pane.close` is worse than it looks and the deck treats it accordingly. It cascades — the last
pane closes the tab, the last tab closes the workspace — and returns a bare `{"type":"ok"}` for
all three, so nothing downstream can report what was actually destroyed. Its one guard,
`confirmation_required`, fires only when herdr's own `confirm_close` is on and a worktree group
would go, and it has a side effect: herdr opens its `ConfirmClose` modal. The deck surfaces that
as its own message on the key and leaves the dialog alone. It could dismiss it —
`agent.focus` settles herdr's mode and would take the modal with it — but answering a question the
user has not read yet is exactly what this project does not do, and it would drag them to another
pane on the way.

## Structure: layouts, worktrees, workspaces and tabs

Two of these methods are traps rather than features, and the deck's shape here is mostly about
not falling into them.

**`layout.apply` with a `tab_id` is destructive, and nothing in its name says so.** It builds the
replacement tab first and then *closes* the tab you named, killing every terminal in it — no
confirmation, no worktree-group guard, no dry run. herdr's own docs put it plainly: it "does not
preserve live PTYs, scrollback, or running processes." Without a `tab_id` the same call is purely
additive: a new tab in the workspace you are in. The deck's client offers no way to supply one, so
a layout preset can only ever add.

**`worktree.remove` is the only call the deck makes that could delete a file.** With `force: true`
it does two destructive things in order: it kills every terminal in the workspace *before* git
runs — so a git failure loses your agents for nothing — and then deletes uncommitted and untracked
files. The deck's client takes no `force` argument and always sends `false`. Unforced, git refuses
to remove a checkout with changes in it, herdr passes that refusal straight back, and the key
shows it. That refusal *is* the confirmation dialog, and it is a better one than a deck could
draw. Committed work is never at risk either way: herdr does not delete the branch.

`worktree.open` is the best-behaved method in the whole API and the deck leans on it: idempotent,
synchronous, non-destructive, and it reports `already_open` rather than making a second workspace.
Its parameters come straight out of `worktree.list`, so nothing has to be typed. The deck opens by
`path` rather than `branch` because a detached checkout has no branch, and a list where some rows
worked and others did not would be worse than a shorter list.

`worktree.create` and `worktree.remove` are the **only two methods in herdr's API that do not
answer at once** — they are deferred until git finishes, which on a large repository is seconds.
The daemon carries them out alongside its event loop rather than inside it, so the key reports
late and the rest of the deck keeps working. Everything else here answers immediately.

`tab.close` cascades the way `pane.close` does, one level up: the last tab of a workspace takes
the workspace with it, and herdr answers the same bare `ok` for both. Like `pane.close` it is
guarded by a hold, never appears in a derived layout, and surfaces `confirmation_required` as a
message pointing at herdr's own window.

`worktree.list` shells out to `git worktree list --porcelain`, so the deck reads it on a slow timer
rather than with every reconcile — and immediately whenever the set of workspaces changes, which is
what opening, creating or removing a worktree always does. Bare and prunable entries are dropped
before they reach a key: herdr answers `worktree_not_found` for both, and a key that cannot work is
worse than a key that is not there.

### Why there is no key that closes a workspace

`workspace.close` is absent from the deck's vocabulary, and there is a test asserting it stays
absent. It is the worst-behaved method in this part of the API:

- **No confirmation of any kind.** `tab.close` and `pane.close` both refuse with
  `confirmation_required` when closing would take a worktree group with it. `workspace.close` has
  no such guard — the TUI's own path honours `confirm_close`, and the API bypasses it entirely.
- **It does not necessarily close one workspace.** When the target is the source-repo member of a
  worktree group with two or more members, it closes *every* workspace in that group: the repo and
  all its worktree children, in one call.
- **It cannot report what it did.** The response is a bare `{"type":"ok"}`, so a key could not tell
  you afterwards whether it had closed one workspace or five.

A daemon could in principle establish that a target is not worktree-grouped before sending — the
`worktree.list` response carries `open_workspace_id` for every checkout. It would rest on inferring
herdr's grouping rule rather than reading it, and it would race: the check and the close are two
calls, and a worktree opened in between makes the answer stale. A wrong inference costs somebody
several workspaces of running agents, so this stays out until herdr offers either a guard of its
own or a response that says what went.

`workspace.rename` and `tab.rename` are out for a duller reason: both require a `label` with no
clear or reset form, and a deck has no keyboard. See
[configuration](configuration.md#presets-are-how-text-reaches-herdr).

## Why herdr-deck doesn't approve prompts

**herdr-deck's write surface to herdr is focus and structure.** The table above is not a partial
list — it is every method the daemon calls. There is no `agent.approve`, no `pane.send_keys`, no
structured yes/no of any kind. A key can turn red the moment `session.snapshot` reports an agent
`blocked` on approval; it deliberately cannot make that need go away. This is not an
unimplemented feature. It is a refusal, and the reasoning is worth writing down so it does not
look like an oversight next to tools whose feature table has an Approve row and ours doesn't.

The blocker isn't hardware — a key is exactly as capable of sending `y\n` into a pane as a phone
button is of POSTing a reply. The blocker is what answering blind means, and three pieces of
prior art make the case better than we could from first principles alone.

- **[collie](https://github.com/AltanS/collie) 0.3.0** (2026-07-03) shipped one-tap approve/deny
  as push-notification quick replies: "Needs-you pushes now carry up to two quick-reply action
  buttons... Tapping one POSTs the reply straight from the service worker... no app open needed."
  Six days later, **0.9.1** (2026-07-09) removed them under a `### Security` heading: "Removed
  one-tap yes/no reply buttons from push notifications — they POSTed to the terminal without
  opening the app, i.e. approving blind." That sentence is the whole argument: a button that
  answers before you've read the question isn't approval, it's a guess wearing your name.
- **collie [ADR 0009](https://github.com/AltanS/collie/blob/main/.adr/0009-a-generic-menu-is-driven-by-the-keys-it-names.md)**
  (accepted 2026-08-05) goes further and bans *digit* keys standing in for numbered menu rows,
  after live-probing Claude Code's `/model` picker on a real pane: pressing a digit didn't just
  pick a model, it "**Set model to Haiku 4.5 and saved as your default for new sessions**" — a
  side effect the picker's own UI never announced anywhere. A key face cannot show you what
  pressing it will actually do to a screen it doesn't fully understand, and herdr-deck's key
  glyphs are no more informative than collie's digit buttons were.
- **[paultyng/agentsd](https://github.com/paultyng/agentsd)** does ship a working Approve key —
  but only because it has something answerable to press it against. Claude Code's
  `PermissionRequest` hook "hold[s] the HTTP response open (up to 120 s) so you can approve or
  deny directly from a button press," with a documented "Permission timeout: 120s. Auto-denies if
  no response." That's a *held, answerable request* with a defined shape and a defined timeout —
  a real primitive to answer, not a blind keystroke into a terminal that may not even be showing
  the prompt by the time it arrives.

herdr has no equivalent primitive. If herdr-deck tried to answer a prompt through the surface it
actually has, the only mechanism available is keystroke injection into a pane — the same blind
mechanism collie shipped in 0.3.0 and deleted six days later for exactly this reason.

### Worse than the thing that already got deleted

A deck key is a worse instrument for this than the phone notification collie removed, not a
better one. Even the smallest quick-reply button in a notification tray sits next to text you
could, in principle, glance at first. A Stream Deck key's icon is a 72–120px square (see
[Hardware](../concepts/hardware.md)) with room for a glyph and a sliver of label, not a prompt.
If herdr grew an answerable primitive tomorrow, wiring a deck key straight to "approve" would
reproduce the exact failure collie already shipped and reverted — just with less screen than the
UI that was judged too blind to keep.

### This is a product boundary, not a waiting room

An earlier draft of this section framed the position as conditional: that an answerable, held
permission primitive upstream — structurally like Claude Code's `PermissionRequest` hook, with a
bounded timeout and a defined auto-deny — would unlock a deck approval key. That framing was
wrong, and no such request has been filed.

herdr-deck controls **herdr**. It shows you which agent needs you and takes you there; the
conversation with the agent happens where you can read it. Approving, denying, replying and
interrupting are all interaction with the agent inside a pane, and they stay out of scope even
if herdr grows a primitive that would make them technically safe. The deck's job ends at the
moment your attention arrives.

That boundary is also what keeps this project from competing with the tools in the
[herdr ecosystem](https://github.com/topics/herdr-plugin) that exist precisely to drive agents
from away-from-desk surfaces. They solve "answer it from wherever I am". herdr-deck solves
"notice it instantly and get there", which is a different problem and wants a different device.

Write access to herdr itself — workspaces, tabs, panes, layouts — is a separate question and is
squarely in scope; it is herdr-centric control, not agent interaction.

## Subscribing

`pane.agent_status_changed` exists in **two** forms: a per-pane filtered subscription that
requires a `pane_id`, and an unfiltered broadcast. herdr-deck uses the broadcast form —
`{"type": "pane.agent_status_changed"}` with no filter — so it sees every pane without
enumerating them first. Adding a `pane_id` would silently limit you to one pane.

## Identifiers

- `pane_id` (`w1:p1`) **changes** when a pane moves between workspaces.
- `terminal_id` (`term_abc123`) is stable across moves.

Bind anything durable to `terminal_id`. But note `agent.focus` accepts an agent name or a *pane
id* — not a terminal id — so resolve at the moment of use.

## Gotchas found the hard way

- The CLI spells a read source `recent-unwrapped`; the **wire enum is `recent_unwrapped`**.
- `report_agent` cannot write `done`. The reportable states are `idle|working|blocked|unknown`;
  `done` is derived by herdr from idle-and-unseen.
- Focusing marks a tab seen. Reading state through the CLI does not.
- `agent.focus` chains the **whole** path itself — workspace, then tab, then pane — and follows
  zoom while it does. Sending `workspace.focus` or `tab.focus` around it to "prepare the way"
  makes three round trips of a journey herdr does atomically, and fights its own logic.
- `workspace.metadata_updated` does not trigger plugin event hooks.
- Pane ids are **positional if they are numeric**: `parse_workspace_id` falls back to treating a
  bare `"2"` as "the second one right now". Never send a positional id from a deck — and never
  keep a pane id anywhere it could go stale, which is why the deck's close-pane key resolves one
  at the moment it is held.

## Version compatibility

herdr-deck compares the `protocol` number from `session.snapshot` against the version it was
built for and **warns rather than refuses** on a mismatch. The wire types ignore unknown fields
and fall back on unknown enum values, so an additive herdr change is harmless — and refusing to
start would be a worse outcome for a status display than rendering one field it does not
understand.

`herdr-deck doctor` reports the mismatch. To check what your herdr actually speaks:

```sh
herdr api schema --json | jq '{protocol, schema_version}'
```
