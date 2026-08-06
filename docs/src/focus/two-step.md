# The two-step focus

Pressing a key that says "this agent needs you" has to do two separate things:

1. **Switch herdr's active pane.** herdr does this — `agent.focus` over its socket.
2. **Bring the terminal window to the front.** herdr does *not* do this. herdr-deck does.

Step 1 works everywhere. Step 2 depends on your desktop.

## Why herdr cannot do step 2

herdr has no API to raise, activate, or bring forward the terminal window hosting it. The only
place herdr performs OS-level window activation is its macOS notification path — it can activate
the terminal when you *click a notification*, and that is all.

So if herdr is attached in a terminal on another desktop, `agent.focus` alone switches herdr's
internal view and nothing visibly happens. herdr-deck fills that gap per platform.

## A useful side effect of step 1

Focusing marks the tab **seen**, which flips a `done` agent to `idle`. Pressing the key both
takes you to the agent and clears it from your attention queue.

## Finding the right window

Raising "a Ghostty window" is not good enough if you have four open. So herdr-deck asks herdr to
stamp a unique marker into its terminal's window title (`client.window_title.set`), then asks the
window manager for the window whose title contains that marker.

This finds the exact window hosting herdr. If the marker cannot be set — herdr has no attached
client yet, say — herdr-deck falls back to raising the application.

On macOS the marker is skipped entirely, because macOS offers no supported way to raise one
specific window of another application. Setting it would rewrite your terminal title for no
benefit.

## When step 2 fails

herdr-deck never pretends. If the window did not come forward:

- the key flashes an alert,
- the daemon logs the specific reason,
- and step 1 has still happened, so the agent is waiting for you when you get there.

`herdr-deck doctor` reports which backend was selected and what it can and cannot do.

## Turning it off

If you would rather keep window focus under your own control:

```toml
[focus]
raise_window = false
```

Pressing a key then only switches herdr's pane.
