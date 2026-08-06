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
| `client.window_title.set` | Stamp a marker so the exact terminal window can be found. |
| `ping` | Liveness. |
| `notification.show` | Optional toast; its `reason` doubles as "is a TUI attached". |

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
