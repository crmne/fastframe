//! Log to stderr and a file for bug reports, keep private data out, and record
//! panics without their payload.
//!
//! A desktop app launched from a menu has no terminal, so its stderr goes
//! nowhere; the log file is what users attach to bug reports. It therefore
//! must never hold message contents, phone numbers, keys, tokens, or pairing
//! payloads. This crate sets up the logger the way ZapFast and Spotifast do:
//!
//! - every line goes to stderr and, when asked, to a file created fresh for
//!   this run;
//! - `RUST_LOG` overrides the app's default filter;
//! - an optional [`Redactor`] rewrites the lines of chosen targets (a
//!   protocol library that quotes what it received, say) before they are
//!   written;
//! - a panic hook appends one line per panic to a panic log, with the time,
//!   the app's version, the thread and the source location, and never the
//!   panic's payload, which can quote whatever data the code was handling.
//!
//! See [`Logging`] for an example.
//!
//! The app's name and version are passed in: this crate's own version is not
//! the app's.
//!
//! For the words that must not reach a line in the first place, see
//! [`redact`].
//!
//! # Apps on `tracing`
//!
//! [`Logging`] installs a [`log`] logger, behind the default `logger`
//! feature. [`log_panics`] and [`redact`] use only the standard library and
//! work with any facade, so an app that logs through `tracing` (RekordFlash)
//! takes them without the logger:
//!
//! ```toml
//! fastframe-log = { git = "...", rev = "...", default-features = false }
//! ```
//!
//! [`log_panics`] replaces the panic hook without chaining the previous one
//! (the default hook would print the payload). Install it first; a hook set
//! later that chains `std::panic::take_hook()`, such as a diagnostics
//! recorder, then runs before it.
//!
//! There is no `tracing` subscriber here: only one app uses `tracing`, and it
//! writes no log file.

#[cfg(feature = "logger")]
mod logger;
mod panic;
pub mod redact;

#[cfg(feature = "logger")]
pub use logger::{Logging, Redactor};
pub use panic::log_panics;
