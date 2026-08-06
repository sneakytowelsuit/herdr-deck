//! `herdr-deckd` — the herdr-deck daemon.
//!
//! Owns everything: the connection to herdr, the state cache, tile rendering, the layout, and
//! the focus engine. Frontends (the macOS Stream Deck plugin, the Linux HID driver) connect to
//! its unix socket and do nothing but forward input and draw what they are given.
//!
//! The daemon is a library with a thin binary on top purely so the seam where these pieces meet
//! — a real socket, a real watcher, a real session — can be driven from an integration test.

pub mod server;
pub mod session;
pub mod watcher;
