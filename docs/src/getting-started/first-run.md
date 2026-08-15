# First run

Start herdr, launch a couple of agents, and watch the deck.

## What you should see

Within about two seconds of an agent changing state, its key changes colour. With nothing
running yet, every key is dark and the attention key reads `0 / all clear`.

If every key instead shows a warning triangle and "herdr not running", the daemon cannot reach
herdr — see [Troubleshooting](../help/troubleshooting.md).

## Try the thing it exists for

1. Give an agent a task that will ask you for approval.
2. Wait for its key to turn **red**.
3. Press that key.

herdr switches to that agent's pane, and your terminal comes to the front — switching desktop or
workspace if it has to.

If the terminal does *not* come forward, the key flashes an alert and the daemon logs why. That
is usually one of:

- you are on [GNOME under Wayland](../focus/gnome-wayland.md), which blocks this;
- `wmctrl`/`xdotool` is not installed on X11;
- your terminal is not the one herdr-deck is looking for — see
  [Configuration](../reference/configuration.md#focus).

Either way herdr itself still switched panes, so the agent is waiting for you when you get there.

## Then try the rest of it

The bottom-right key is the **page key**. Press it a few times: the deck walks from agents to
workspaces, to pane control, to the keys that make a tab or a worktree. The key says which page you
are on, so you never have to count.

Then press the **attention key** next to it. Wherever you have got to, it brings the deck home —
and if anything is asking for you, it takes you there in the same press. That is worth doing once
deliberately, because it is the thing that makes the rest of the control surface safe to wander
around in. [What each page can do.](../reference/control-families.md)

## Running it in the foreground

While you are getting set up, it is often easier to watch the daemon directly than to read
service logs:

```sh
herdr-deck service stop
herdr-deckd --log debug
```

It prints which herdr socket it chose, which window-raise backend it detected, and every frontend
that connects.
