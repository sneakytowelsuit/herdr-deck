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

## Why herdr-deck doesn't approve prompts

**herdr-deck's entire write surface to herdr is focus.** The table above is not a partial list —
it is every method the daemon calls. There is no `agent.approve`, no `pane.send_keys`, no
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
