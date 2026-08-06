# Hardware bring-up

A runbook for the first time you plug a Stream Deck into a Linux box running herdr-deck.

**Read this first: none of this has been run against real hardware.** herdr-deck's Linux HID
path — including `--selftest` itself — was built and tested entirely through hidapi's type
signatures, golden-image renders, and a mocked herdr protocol, on a machine with no Stream Deck
attached. That is exactly the situation this runbook exists to fix, but it means the runbook
itself is unverified. If a step below does not match what you see, believe your hardware, not
this page, and please say so — see [Contributing](./contributing.md).

If you are on macOS, this page is not for you: the Elgato app owns the device there, and bring-up
means installing the plugin. See [Install](../getting-started/install.md#macos).

## Why go through this instead of just starting the daemon

If you plug in a deck, start `herdr-deckd`, and nothing happens, the failure could be in any of:
udev permissions, the HID driver, the image format for your model, key numbering, dial polling,
or touchstrip placement — and the normal daemon changes several of those at once, with herdr and
the frontend protocol in the mix too. Each step below isolates one layer, so a failure narrows
down to one place instead of "something, somewhere, in the whole stack."

## Step 1 — udev

The device needs a rule granting the logged-in user access; without it, only root can open it.

```sh
sudo tee /etc/udev/rules.d/70-herdr-deck.rules >/dev/null <<'EOF'
# herdr-deck: let the logged-in user talk to Elgato Stream Deck hardware.
SUBSYSTEM=="usb", ATTRS{idVendor}=="0fd9", TAG+="uaccess"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0fd9", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then **unplug and replug the deck** — the rule only applies to devices enumerated after it loads.

**Working looks like:** `lsusb | grep -i 0fd9` shows the device, and
`ls -l /dev/hidraw*` shows one owned by your user (via the `uaccess` ACL, not a fixed group).

**If the device is not there at all:** cable or hub problem, not udev — try a different port.

**If it is there but you still get a permissions error in the next step:** the rule did not
apply. Most likely you edited the file but skipped `udevadm trigger`, or skipped the replug —
`uaccess` is granted at the moment the device is enumerated, not retroactively. Also check you
are at a real logind seat (an SSH session with no active graphical login has no seat for
`uaccess` to hand the device to).

## Step 2 — `herdr-deckd --selftest`

This talks to the deck directly. No config file, no herdr, no daemon socket.

```sh
herdr-deckd --selftest
```

**Working looks like** output resembling:

```
herdr-deck self-test: talking to the hardware directly, no daemon, no herdr.

found 1 device(s):
  - Plus (serial ABC12345)

opening Plus (serial ABC12345)...
firmware: 1.02.006
geometry: 4x2 keys (8 total, 120px), 4 dial(s), touchstrip 800x100

painting 8 key(s)...
done. Keys are numbered 0..7, left-to-right then top-to-bottom — the same order the daemon
addresses them in. Check each physical key shows the number you expect.

painting 4 touchstrip segment(s)...
done. If two dial numbers overlap, or one segment is missing, the segments are stacking on top
of each other rather than tiling — that is exactly the bug this step exists to catch.

now press every key, press and twist every dial, and tap the touchstrip. Ctrl-C to exit and
reset the deck.
```

**If it says `no Stream Deck found on USB`:** back to Step 1 — this is almost always udev.

**If it fails to open the device with a permissions error:** same — the udev rule did not take.

**If it opens but `firmware: unknown`:** harmless. Some firmware versions do not answer that
query over hidapi; it does not block anything else here.

Keep it running for the next two checks — it is still painting and reading input.

## Step 3 — check key numbering

Look at the deck. Every key should show a distinct number, a two-letter `r`/`c` position (e.g.
`r1c3` for row 1, column 3), and a different background colour from its neighbours.

**Working looks like:** the numbers read in order left-to-right, then top-to-bottom, starting at
`0` in the physical top-left key. That is the same order `herdr-deck-hid` reports key presses in,
and the same order the daemon lays agents out in — this is the check that they agree.

> Note the off-by-one: this tool numbers from `0`, matching the daemon's internal addressing.
> [What the default layout does](../getting-started/default-layout.md) describes controls as
> "key 1" through "key 8" for a human reading the docs. Key `0` here is "key 1" there.

**If the numbers are jumbled or mirrored:** this would be a real bug in the image
format/rotation/mirroring for your specific model — worth filing upstream, since it means every
key image drawn by the real daemon is equally wrong on your hardware.

**If a key stays blank or black:** check the terminal output for `could not paint key N` — that
error includes the underlying HID failure. A single key failing while the rest work usually means
a flaky write, not a systemic problem; if every key fails, suspect the image size/format for your
model (`key_image_px` in the printed geometry line should match your model's native key size).

## Step 4 — check dial and touchstrip mapping

Skip this step if your model has no dials (only the Stream Deck + and Plus XL do).

Look at the touchstrip. It should show `dial 0`, `dial 1`, and so on, evenly tiled with no
overlap, each on its own background colour.

**Working looks like:** one clearly separated segment per dial, numbered in order.

**If segments overlap or one is missing:** the segment math (`strip_width / dial_count`) is
placing images at the wrong x offset for this device's actual strip width — compare the
`touchstrip WxH` figure in Step 2's geometry line against what you expect for your model (800×100
for the Stream Deck +). A mismatch there is the bug to chase.

Now exercise the controls one at a time and watch the terminal:

- **Press each key.** You should see exactly one `key N down` followed by `key N up` per press,
  with `N` matching the number painted on that key.
- **Press each dial.** `dial N down` / `dial N up`, `N` matching its position left-to-right.
- **Twist each dial** a small amount in each direction. You should see `dial N twist ±T
  (clockwise|counter-clockwise)`. The self-test does not assume which physical direction is
  positive — read the label against which way you actually turned it, since that is exactly what
  this step is for confirming.
- **Tap the touchstrip** in a few places along its width. You should see `touch (x, y) -> segment
  for dial N`, with `N` matching the segment you tapped, not a neighbour.

**If a control produces no output at all:** either it is not being polled (a HID read problem —
try unplugging and replugging) or the physical control is faulty. Comparing against a different
key/dial on the same deck tells you which.

Press Ctrl-C now. You should see `Ctrl-C received, resetting the deck...` followed by `deck
reset. self-test complete.`, and the deck should go dark. If it does not reset, power-cycle it
(unplug and replug) before moving on — a stale image left on the keys is otherwise harmless, but
best not to confuse with the next step's output.

## Step 5 — the full stack

Now bring in the daemon (and herdr, if you have it running):

```sh
herdr-deck service stop     # in case it's already running
herdr-deckd --log debug
```

**Working looks like:** log lines showing `opened deck` with the same geometry Step 2 printed, a
`listening for frontends` line, and every key lighting up — either real agent tiles if herdr is
running, or the dim "`0 / all clear`" attention tile and offline tiles if it is not. See
[First run](../getting-started/first-run.md) for what that should look like.

**If the deck stays dark:** the daemon could not open it — since Step 2 just proved the device
and udev rule both work, look for `no Stream Deck found` or a permissions error in the daemon's
own log output; it is a separate process, so it needs the same udev grant, but there is no reason
it should behave differently from Step 2 unless something else has the device open (unplug
`--selftest` if it is still running — only one process can hold the device at a time).

**If keys light up but every key says "herdr not running":** that is herdr, not the deck — see
[Troubleshooting: every key says "herdr not running"](./troubleshooting.md#every-key-says-herdr-not-running).

## Step 6 — the end-to-end check

This is the one that matters: does pressing a key actually get you to the agent that needs you.

1. Start (or already have running) an agent that will hit an approval or a question.
2. Wait for its key to turn **red**, with a `!` glyph.
3. Press that key.

**Working looks like:** herdr switches to that agent's pane, and your terminal window comes to
the front — even across a desktop or workspace switch.

**If the key never turns red:** herdr has not classified the agent as `blocked`. The deck is only
ever as accurate as herdr's own detection — see
[`blocked` and `done`](../concepts/attention.md#an-important-caveat-about-detection) and
`herdr agent explain <target>`. This is not a hardware problem, even though it shows up on the
hardware.

**If the key works but the terminal never comes forward:** herdr *did* switch panes — the deck
did its job — but window raising failed. That is covered in
[Troubleshooting: keys work, but the terminal never comes forward](./troubleshooting.md#keys-work-but-the-terminal-never-comes-forward),
and it is a real, sometimes unfixable case on GNOME/Wayland — see
[GNOME on Wayland](../focus/gnome-wayland.md).

If Step 6 works, bring-up is done — the whole chain from `blocked` to a key you can press to a
terminal in front of you has been checked at every layer, not just assumed.
