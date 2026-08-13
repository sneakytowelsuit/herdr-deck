# GNOME on Wayland

**On GNOME under Wayland, herdr-deck cannot raise your terminal window.** Pressing a key still
switches herdr to the right agent — you will just have to alt-tab to the terminal yourself.

This is not a bug in herdr-deck, and it is not something a future version can quietly fix.

## Why

GNOME's Wayland compositor (Mutter) deliberately does not expose a way for one application to
activate another's window. There is no D-Bus method, no CLI, no protocol extension in the default
session. The restriction is a focus-stealing-prevention design decision, and it applies to every
tool, not just this one.

`wmctrl` and `xdotool` appear to run under GNOME Wayland because XWayland is present, but they
can only see X clients. A native-Wayland terminal is invisible to them.

## What herdr-deck does about it

It tells you, rather than silently doing half the job:

- `herdr-deck doctor` reports `window raising: no usable backend` and explains why;
- the daemon logs the same at startup;
- pressing a focus key flashes an alert on that key.

herdr's own pane focus still happens every time.

## Your options

### Use a GNOME X11 session

The simplest fix. Log out, and on the login screen choose **GNOME on Xorg**. herdr-deck then
detects the `x11` backend and window raising works normally, provided `wmctrl` or `xdotool` is
installed.

### Use a compositor with scriptable IPC

Hyprland and Sway both expose window activation and work out of the box. KDE Plasma on Wayland
works through KWin scripting.

### Add a GNOME extension that exposes activation

Extensions such as *Window Calls* add a D-Bus interface for activating windows. If you use one,
you can point herdr-deck at a custom command — but there is no built-in backend for this, because
the extension ecosystem is too unstable to depend on.

### Accept it

Honestly, this is fine for many people. The deck still tells you *which* agent needs you and
still switches herdr to it. You alt-tab, and the right pane is already in front of you.

### Turn the attempt off

To stop the alert flashing on every press:

```toml
[focus]
raise_window = false
```
