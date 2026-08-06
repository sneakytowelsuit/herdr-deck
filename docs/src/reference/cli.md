# CLI

Two binaries: `herdr-deck` (the tool you run) and `herdr-deckd` (the daemon).

## `herdr-deck`

### `doctor`

The one command to run when something is wrong. It checks the config, herdr's socket and
protocol version, whether the daemon is running, whether a deck can be seen, which window-raise
backend was selected, and whether its helper tools are installed.

```console
$ herdr-deck doctor
[ok  ] config: no file at /home/you/.config/herdr-deck/config.toml — using defaults
[ok  ] herdr socket: /home/you/.config/herdr/herdr.sock (via default session)
[ok  ] herdr protocol: protocol 19, 4 agent(s) visible
[ok  ] herdr-deckd: running, socket /run/user/1000/herdr-deck.sock
[ok  ] deck: Stream Deck + — 4x2 keys (120px), 4 dials, touchstrip 800x100 (serial A00000000000)
[ok  ] window raising: backend `hyprland`
[ok  ] window tools: found hyprctl
[ok  ] terminal: targeting `com.mitchellh.ghostty`

Everything looks good.
```

Every problem it reports comes with the command that fixes it. It exits non-zero only on a real
failure — a warning (say, GNOME Wayland) still exits `0`, so a health check does not call a
working deck broken.

### `status`

What herdr currently reports, in the same order the deck uses.

```sh
herdr-deck status
herdr-deck status --json    # includes terminal_id, for pinning keys
```

### `layout`

What each control would do on a given deck. Needs no hardware.

```sh
herdr-deck layout --model plus
herdr-deck layout --model xl
```

Models: `plus`, `original`/`mk2`, `mini`, `xl`, `neo`, `pedal`.

### `install`

Writes a starter config, then checks that the hardware can actually be reached.

```sh
herdr-deck install [--force]
sudo herdr-deck install --write-udev
```

| Flag | Meaning |
|---|---|
| `--force` | Overwrite an existing config. Without it, an existing config is left alone — but the hardware check still runs. |
| `--write-udev` | Linux only. Install the udev rule to `/etc/udev/rules.d/` and reload udev. Needs root; without it, `install` says so and changes nothing. |

On Linux it enumerates attached decks and reports the model, its keys and its dials:

```console
$ herdr-deck install
wrote /home/you/.config/herdr-deck/config.toml
...
Found 1 deck:
  Stream Deck + — 4x2 keys (120px), 4 dials, touchstrip 800x100 (serial A00000000000)
```

When it finds nothing it says what that might mean rather than stating it as fact, because
without the udev rule an attached deck is invisible to enumeration:

```console
no deck found — but the udev rule is missing, which alone would hide one

Install the rule with `sudo herdr-deck install --write-udev`, then unplug and replug the deck.
...
```

Without `--write-udev` the rule is printed rather than installed — writing under `/etc` stays
opt-in. On macOS there is nothing to enumerate: Elgato's app owns the device, so `install` says
so and points at the [plugin step](../getting-started/install.md#macos) instead.

### `service`

Manages the daemon under `launchd` (macOS) or `systemd --user` (Linux).

```sh
herdr-deck service install     # write the unit and start it
herdr-deck service start|stop|restart|status
herdr-deck service show        # print the unit without writing it
herdr-deck service uninstall
```

### `icons`

Regenerates the Stream Deck plugin's icon set from the theme. A build-time tool.

```sh
herdr-deck icons --out plugin/com.sneakytowelsuit.herdr-deck.sdPlugin/imgs
```

## `herdr-deckd`

Normally started by the service manager. Run it directly to watch what it is doing:

```sh
herdr-deckd --log debug
```

| Flag | Meaning |
|---|---|
| `--config PATH` | Config file. |
| `--socket PATH` | Frontend socket path. |
| `--session NAME` | herdr session to attach to. |
| `--log FILTER` | Log filter (also `HERDR_DECK_LOG`). |
| `--dry-run DIR` | Render the layout to PNGs and exit. |
| `--dry-run-model M` | Hardware to assume for `--dry-run`. |
| `--selftest` | Talk to the first attached deck directly and exit. Linux only. |

### `--dry-run`

Renders every key and dial image for a model into a directory, using a sample state that
exercises all five agent statuses. No herdr, no hardware, no deck:

```sh
herdr-deckd --dry-run /tmp/tiles --dry-run-model plus
```

Useful for checking a theme change, and how the docs and CI get their images.

### `--selftest`

The opposite of `--dry-run`: real hardware, but no herdr and no daemon logic. Opens the first
deck it finds over HID, paints a numbered test pattern on every key and touchstrip segment, then
prints every input event as you press things, until Ctrl-C resets the deck and exits.

```sh
herdr-deckd --selftest
```

For first-time hardware bring-up — see [Hardware bring-up](../help/hardware-bringup.md). Linux
only: on macOS the Elgato app owns the device, so there is nothing for this to open; use the
Stream Deck plugin instead.
