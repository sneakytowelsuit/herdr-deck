# Install

herdr-deck is two pieces:

- **`herdr-deckd`**, a daemon that watches herdr and decides what the deck shows;
- **a frontend** that drives your hardware — the Stream Deck plugin on macOS, or a built-in USB
  HID driver on Linux.

You need herdr 0.8.0 or newer.

## Via herdr

The easiest route, since herdr builds and registers it for you:

```sh
herdr plugin install sneakytowelsuit/herdr-deck --yes
herdr plugin action invoke install --plugin sneakytowelsuit.herdr-deck
```

That is the whole install on both platforms. The first command builds the daemon and, **on
macOS**, also builds the Stream Deck plugin and links it into Elgato's app; the second writes the
service unit and starts the daemon. On Linux there is no Elgato app to link into — `herdr-deckd`
drives the deck itself — so the first command has nothing extra to do, and only the udev rule
below is left.

### Updating

The same first command again:

```sh
herdr plugin install sneakytowelsuit/herdr-deck --yes
herdr plugin action invoke restart --plugin sneakytowelsuit.herdr-deck
```

herdr re-clones, rebuilds, and swaps the result into the same place. The Stream Deck plugin is
*linked* rather than copied, so the app picks up the rebuilt code without being touched again.

herdr has no `plugin update` command and does not check for new versions on its own — build steps
run at install time and nowhere else — so this is the update, and it is a deliberate re-run rather
than something that happens behind you.

### If the Stream Deck step is skipped

Linking is best effort: it will never abort the install and leave you with no daemon, which is the
half that does the work. If Node or the Stream Deck app is missing it says so and carries on, and
`herdr-deck doctor` reports `stream deck plugin: not linked` until you fix it and re-run the
install.

`invoke` is not optional: `herdr plugin action <something>` only understands `list` and `invoke`,
and anything else prints the help text and exits without doing a thing — quietly enough to look
like it worked. The other actions are reached the same way, and `herdr plugin action list` shows
them all:

```sh
herdr plugin action invoke status  --plugin sneakytowelsuit.herdr-deck
herdr plugin action invoke restart --plugin sneakytowelsuit.herdr-deck
herdr plugin action invoke doctor  --plugin sneakytowelsuit.herdr-deck
```

`--plugin` can be dropped when no other installed plugin defines an action of the same name, but
`install` and `status` are common words; naming the plugin costs nothing and never surprises you.

## From source

```sh
git clone https://github.com/sneakytowelsuit/herdr-deck
cd herdr-deck
cargo build --release -p herdr-deckd -p herdr-deck-cli
./target/release/herdr-deck service install
```

## macOS

The daemon does not talk to the hardware on macOS — Elgato's app owns the device — so a Stream
Deck plugin does the drawing. Installing via herdr builds and links it for you; this section is
what that automation does, and what to run if you are working from a source checkout.

1. Build and link it:

   ```sh
   scripts/install-streamdeck-plugin.sh
   ```

   That builds the plugin and runs `streamdeck link`, which **symlinks** it into
   `~/Library/Application Support/com.elgato.StreamDeck/Plugins/`. The symlink is the reason
   updates need nothing more than a rebuild.

2. In the Stream Deck app, drag **herdr Agent** onto every key you want herdr-deck to use, and
   **herdr Dial** onto each dial. These are two separate actions — placing the key action does
   not give you the dials.

You do not have to configure the keys. The daemon decides what each one shows, so a key placed
anywhere just works.

> **Why do I place the action on every key myself?**
> Because herdr-deck only drives controls you have given it. That is deliberate — it means you
> can hand it half a deck and keep the rest for something else, and herdr-deck will lay itself
> out in the space it actually has.

## Linux

The daemon drives the deck directly, so there is nothing else to install — but the device needs
a udev rule to be reachable without root.

```sh
sudo herdr-deck install --write-udev
```

That writes `/etc/udev/rules.d/70-herdr-deck.rules`, runs `udevadm control --reload-rules &&
udevadm trigger`, and then looks for your deck and tells you what it found. Unplug and replug
the deck afterwards — udev applies a rule to devices that appear *after* it is loaded.

Run without root, it refuses and says so rather than dying on a permission error. Run without
`--write-udev`, it prints the rule instead of installing it: writing under `/etc` stays
something you ask for explicitly.

<details>
<summary>The same thing by hand</summary>

```sh
sudo tee /etc/udev/rules.d/70-herdr-deck.rules >/dev/null <<'EOF'
# herdr-deck: let the logged-in user talk to Elgato Stream Deck hardware.
SUBSYSTEM=="usb", ATTRS{idVendor}=="0fd9", TAG+="uaccess"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0fd9", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

</details>

> **"No Stream Deck found" and "wrong permissions" look identical on Linux.**
> Without the rule, an attached deck does not show up in enumeration at all — so herdr-deck
> never reports "no deck found" without also telling you whether the rule is in place. Fix the
> permissions first; only then is an empty result evidence that nothing is plugged in.

### Window raising needs a helper on X11

On X11, install `wmctrl` or `xdotool`. Hyprland, Sway and KDE use tools that ship with the
compositor. [`herdr-deck doctor`](../reference/cli.md#doctor) tells you exactly what is missing.

## Check it worked

```sh
herdr-deck doctor
```

Every problem it reports comes with the command that fixes it.
