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
herdr plugin action sneakytowelsuit.herdr-deck install
```

The second command writes the service unit and starts the daemon. Then follow the
platform-specific step below.

## From source

```sh
git clone https://github.com/sneakytowelsuit/herdr-deck
cd herdr-deck
cargo build --release -p herdr-deckd -p herdr-deck-cli
./target/release/herdr-deck service install
```

## macOS

The daemon does not talk to the hardware on macOS — Elgato's app owns the device — so you also
need the Stream Deck plugin.

1. Build it:

   ```sh
   cd plugin
   npm install
   npm run build
   ```

2. Double-click `plugin/com.sneakytowelsuit.herdr-deck.sdPlugin` to install it, or copy that
   folder into
   `~/Library/Application Support/com.elgato.StreamDeck/Plugins/`.

3. In the Stream Deck app, drag **herdr Agent** onto every key you want herdr-deck to use, and
   **herdr Dial** onto each dial.

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
sudo tee /etc/udev/rules.d/70-herdr-deck.rules >/dev/null <<'EOF'
# herdr-deck: let the logged-in user talk to Elgato Stream Deck hardware.
SUBSYSTEM=="usb", ATTRS{idVendor}=="0fd9", TAG+="uaccess"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="0fd9", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then unplug and replug the deck.

`herdr-deck install` prints these commands too. It does not run them for you — installing a file
under `/etc` needs root, and quietly escalating during an install would be a surprise.

### Window raising needs a helper on X11

On X11, install `wmctrl` or `xdotool`. Hyprland, Sway and KDE use tools that ship with the
compositor. [`herdr-deck doctor`](../reference/cli.md#doctor) tells you exactly what is missing.

## Check it worked

```sh
herdr-deck doctor
```

Every problem it reports comes with the command that fixes it.
