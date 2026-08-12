# How herdr maps onto the deck

herdr's model nests like this:

```
Session          a herdr server namespace, with its own socket
└── Workspace    the top-level project container
    └── Tab      a layout inside a workspace
        └── Pane a real terminal
            └── Agent   a process herdr recognises inside that pane
```

An **agent is a property of a pane**, not an object with its own lifetime. When a pane's process
is replaced, the agent goes with it.

## What lands on a key

By default, keys show **agents**, because an agent is the thing that can need you. Pressing the
mode toggle switches the deck to **workspaces**, which is the level you navigate at.

herdr-deck does not model tabs or panes as key targets — dial 3 scrubs tabs and pressing it
focuses the tab you landed on, and that has been enough. Sessions are not modelled either: the
daemon talks to one herdr session, chosen with `--session` or `herdr_session` in config.

## Why keys bind to `terminal_id`

Every pane has two identifiers:

- **`pane_id`** (`w1:p1`) — workspace-qualified, and it **changes** when a pane moves to another
  workspace.
- **`terminal_id`** (`term_abc123`) — stable for the life of the terminal, across moves.

herdr-deck binds deck slots to `terminal_id`, so a key pinned to an agent keeps working after you
reorganise your workspaces.

There is a wrinkle: herdr's `agent.focus` takes an agent name or a *pane id*, not a terminal id.
So the daemon resolves `terminal_id` → `pane_id` against current state at the moment you press
the key. If the agent disappeared between the key being painted and you pressing it, herdr-deck
does nothing rather than focusing whatever moved into that slot.

## Projects, worktrees, remotes

- **Projects** are not a herdr core concept — they come from the `herdr-plus` plugin, so
  herdr-deck does not model them.
- **Worktrees** are ordinary workspaces with git provenance, and appear as workspaces.
- **Remotes** mean SSH attach. herdr-deck assumes herdr and the deck are on the same machine and
  talks to a local socket.
