# Window-raise backends

herdr-deck picks a backend at runtime by reading environment variables, so the same binary works
across machines with different desktops.

## The detection order

| Checked | Signal | Backend |
|---|---|---|
| 1 | macOS | `macos` |
| 2 | `HYPRLAND_INSTANCE_SIGNATURE` | `hyprland` |
| 3 | `SWAYSOCK` | `sway` |
| 4 | a real X11 session | `x11` |
| 5 | Wayland + KDE/Plasma | `kwin` |
| 6 | anything else | `unsupported` |

Compositor-specific signals are checked first because they are unambiguous. X11 is checked before
the Wayland branch because `wmctrl` works there under *any* window manager, including GNOME's and
KDE's X11 sessions — it is the most reliable Linux path.

`DISPLAY` being set under Wayland (XWayland) does **not** count as an X11 session. Raising via
`wmctrl` there would only reach X clients and miss the native-Wayland terminal you actually want.

## What each backend runs

Commands are tried in order; the first that succeeds wins. Nothing is ever passed through a
shell — window titles come from arbitrary terminal output, and putting that in a shell command
would be a command-injection hole.

| Backend | Needs | Commands tried |
|---|---|---|
| `macos` | — | `open -b <bundle-id>` |
| `hyprland` | `hyprctl` | `hyprctl dispatch focuswindow title:…` then `class:…` |
| `sway` | `swaymsg` | `swaymsg '[title="…"] focus'` then `[app_id="…"] focus` |
| `kwin` | `kdotool` or `wmctrl` | `kdotool search --name/--class`, then `wmctrl -x -a` |
| `x11` | `wmctrl` or `xdotool` | `wmctrl -a <marker>`, `wmctrl -x -a <class>`, `xdotool search … windowactivate` |

macOS uses `open -b` rather than AppleScript `activate` deliberately: `open` needs no Automation
(TCC) permission, so there is no scary consent dialog the first time you press a key.

If one helper is missing but another works, herdr-deck uses the one that works. It only reports a
missing tool when none of the alternatives are installed.

## Telling herdr-deck about your terminal

The default is Ghostty:

```toml
[focus]
macos_bundle_id = "com.mitchellh.ghostty"
linux_app_id = "com.mitchellh.ghostty"
```

Common alternatives:

| Terminal | macOS bundle id | Linux app id / class |
|---|---|---|
| Ghostty | `com.mitchellh.ghostty` | `com.mitchellh.ghostty` |
| kitty | `net.kovidgoyal.kitty` | `kitty` |
| WezTerm | `com.github.wez.wezterm` | `org.wezfurlong.wezterm` |
| Alacritty | `org.alacritty` | `Alacritty` |
| iTerm2 | `com.googlecode.iterm2` | — |
| Terminal.app | `com.apple.Terminal` | — |

Find your Linux app id with `hyprctl clients`, `swaymsg -t get_tree`, or `xprop WM_CLASS`.

## Forcing a backend

Mostly a debugging aid:

```toml
[focus]
backend = "x11"   # macos | hyprland | sway | kwin | x11 | unsupported
```

An unrecognised value falls back to detection rather than breaking focus.
