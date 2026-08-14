//! herdr socket client.
//!
//! # Why this crate exists
//!
//! Every fact about herdr's wire protocol — method names, socket paths, JSON shapes, enum
//! spellings — lives in this crate and nowhere else. When herdr changes its protocol, this is
//! the only crate that needs to move. Nothing above it should ever construct a herdr method
//! name or match on a herdr JSON field.
//!
//! # Protocol notes (herdr 0.8.0, socket protocol 19)
//!
//! - Transport is newline-delimited JSON over a unix domain socket.
//! - **RPC is one-shot.** herdr closes the connection after a single response, so every call
//!   opens a fresh connection, writes one line, reads one line, and closes. The only methods
//!   that keep a connection open are `events.subscribe` and `pane.graphics.stream`.
//! - Gate compatibility on the `protocol` number reported by `session.snapshot`, not on the
//!   herdr release version.

pub mod client;
pub mod events;
pub mod socket;
pub mod wire;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use client::HerdrClient;
pub use events::{EventStream, HerdrEvent};
pub use socket::SocketPath;
pub use wire::{AgentInfo, AgentStatus, PaneInfo, SessionSnapshot, TabInfo, WorkspaceInfo};

/// The herdr socket protocol version this crate was written against.
///
/// Reported by `session.snapshot` as `protocol`. We warn rather than refuse on a mismatch:
/// herdr's additive changes are common and usually harmless, and refusing to start would be a
/// worse failure mode for a status display than rendering slightly stale fields.
pub const EXPECTED_PROTOCOL: u32 = 19;

/// Errors surfaced by the herdr client.
#[derive(Debug, thiserror::Error)]
pub enum HerdrError {
    #[error("herdr socket not found at {path} — is herdr running?")]
    SocketMissing { path: String },

    #[error("could not connect to herdr socket at {path}: {source}")]
    Connect {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("herdr closed the connection before sending a response to `{method}`")]
    NoResponse { method: String },

    #[error("herdr returned an error for `{method}`: [{code}] {message}")]
    Rpc {
        method: String,
        code: String,
        message: String,
    },

    /// herdr wants the user to confirm, and has opened its own dialog to ask them.
    ///
    /// Told apart from a plain [`HerdrError::Rpc`] because the two need different words on a key.
    /// An ordinary refusal means nothing happened and nothing is waiting; this one means herdr is
    /// now sitting in a modal the user did not open and the deck has no way to answer or dismiss.
    /// The only honest thing a key can do is say where the question went.
    #[error(
        "herdr is asking you to confirm this in its own window — the deck cannot answer for you"
    )]
    ConfirmationRequired { method: String },

    #[error("could not parse herdr's response to `{method}`: {source}")]
    Decode {
        method: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("i/o error talking to herdr: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, HerdrError>;
