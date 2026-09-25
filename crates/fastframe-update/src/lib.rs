//! Self-update from GitHub releases for fastframe apps.
//!
//! The app describes itself once in an [`UpdateConfig`], gives an
//! [`Updater`] its own HTTP client as a [`Transport`], and keeps every
//! decision about when to check, what to show and how to word it:
//!
//! ```no_run
//! use fastframe_update::{Launch, UpdateConfig, Updater};
//!
//! const UPDATES: UpdateConfig = UpdateConfig {
//!     legacy_names: &["fastsapp"],
//!     // In the app: include_str!("../assets/update-public-key.hex")
//!     publisher_key: Some("c2256da754d1d10b855348fdff0070f4e415424ef9237a39276a3c1e55bdbe35"),
//!     ..UpdateConfig::new("crmne/zapfast", "ZapFast", "zapfast", env!("CARGO_PKG_VERSION"))
//! };
//!
//! # struct MyClient;
//! # impl fastframe_update::Transport for MyClient {
//! #     fn get(&self, _: &fastframe_update::Request<'_>) -> anyhow::Result<fastframe_update::Response> {
//! #         unimplemented!()
//! #     }
//! # }
//! fn main() -> anyhow::Result<()> {
//!     // First thing: run the helper when asked to, and take the update flags
//!     // off the command line before the app parses it.
//!     let launch: Launch = fastframe_update::intercept(&UPDATES);
//!     // ... parse `launch.arguments`, show `launch.error` as a toast ...
//!
//!     // On a worker thread, when the app decides to check:
//!     let updater = Updater::new(UPDATES, MyClient);
//!     if let Some(release) = updater.check()? {
//!         if updater.installation().is_ok() {
//!             let prepared = updater.download(&release, |received, total| {
//!                 // report progress
//!             })?;
//!             // After the user chooses to restart:
//!             updater.handoff(prepared, vec!["--verbose".into()])?;
//!             // Now quit the app; the helper waits for it to exit.
//!         }
//!     }
//!
//!     // After the first frame of a relaunched app:
//!     if let Some(receipt) = launch.receipt {
//!         receipt.acknowledge()?;
//!     }
//!     Ok(())
//! }
//! ```
//!
//! # How an update is installed
//!
//! 1. [`Updater::check`] reads the latest GitHub release and compares it with
//!    the version the app passed in [`UpdateConfig::current_version`].
//! 2. [`Updater::installation`] decides whether this copy may replace itself.
//!    Package-manager installs (Flatpak, Snap, apt, dnf, pacman, Nix,
//!    Homebrew, cargo) are refused with an [`Unsupported`] reason, as is any
//!    Linux or Windows copy without a portable or installer marker file.
//! 3. [`Updater::download`] fetches `checksums.txt`, verifies its Ed25519
//!    signature (`checksums.txt.sig`) against the embedded publisher key,
//!    downloads the package into a fresh staging folder beside the app and
//!    checks its SHA-256. It then unpacks the executable and asks it for its
//!    `--version`, or on macOS checks the disk image's bundle identity,
//!    version and code signature.
//! 4. [`Updater::handoff`] copies the running app into the staging folder as
//!    a helper, writes `handoff.json`, and starts the helper with
//!    `--apply-update`. The app then quits.
//! 5. The helper waits for the app to exit, backs it up as `previous`,
//!    replaces it and relaunches it with `--update-receipt`. The new app
//!    calls [`Receipt::acknowledge`] once its window is up, which writes
//!    `started`. If that never happens within a minute, or anything before
//!    fails, the helper restores `previous` and restarts the old app with
//!    `--update-error`.
//!
//! The helper is always a copy of the app that downloaded the update, and
//! the relaunched app reads the receipt it wrote. The file names, flags and
//! JSON shape are therefore a contract between versions; see the crate
//! README and `tests/fixtures`.

use std::time::Duration;

mod detect;
mod download;
mod helper;
mod host;
mod macos;
mod release;
#[cfg(feature = "reqwest")]
mod reqwest_transport;
mod signing;
mod stage;
mod startup;
mod transport;
mod updater;
mod version;

pub use detect::{Installation, Kind, PackageManager, Unsupported};
pub use release::Release;
#[cfg(feature = "reqwest")]
pub use reqwest_transport::ReqwestTransport;
pub use stage::Prepared;
pub use startup::{Launch, Receipt, intercept};
pub use transport::{Request, Response, Source, Transport};
pub use updater::Updater;
pub use version::is_newer;

/// How often apps check for a newer release: once a day.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The command-line flag that runs the helper: `app --apply-update <job>`.
pub const APPLY_UPDATE_FLAG: &str = "--apply-update";
/// The flag the helper passes to a successfully replaced app.
pub const UPDATE_RECEIPT_FLAG: &str = "--update-receipt";
/// The flag the helper passes, with a message, to a restored app.
pub const UPDATE_ERROR_FLAG: &str = "--update-error";

/// Everything the updater needs to know about the app.
///
/// Build it with [`UpdateConfig::new`] and override the rest with struct
/// update syntax. Every value is `'static`, so it fits in a `const`.
#[derive(Clone, Copy, Debug)]
pub struct UpdateConfig {
    /// The GitHub repository, `owner/name`, for example `crmne/zapfast`.
    pub repository: &'static str,
    /// The display name, for example `ZapFast`. Used in the user agent and
    /// as the macOS bundle name (`ZapFast.app`).
    pub app_name: &'static str,
    /// The lowercase command name, for example `zapfast`. Release assets
    /// (`zapfast-v1.2.3-<target>.tar.gz`), the executable inside them, the
    /// staging folder (`.zapfast-update-<random>`), the marker files
    /// (`zapfast-portable.txt`) and the expected `--version` output
    /// (`zapfast 1.2.3`) all derive from it.
    pub slug: &'static str,
    /// The running app's version: pass `env!("CARGO_PKG_VERSION")` from the
    /// app's own crate. Inside this crate that macro would name fastframe's
    /// version instead.
    pub current_version: &'static str,
    /// Names the app shipped under before, for example `fastpotify`. Each
    /// is accepted wherever the slug is: marker files, `--version` output,
    /// staging folders and Homebrew casks.
    pub legacy_names: &'static [&'static str],
    /// Where the Windows installer put the app before it wrote an installer
    /// marker, relative to `%LOCALAPPDATA%`. The default location,
    /// `Programs/<app_name>/<slug>.exe`, is always checked.
    pub legacy_windows_installs: &'static [&'static str],
    /// The macOS bundle's identity.
    pub macos: MacConfig,
    /// The publisher's Ed25519 public key, 64 hexadecimal characters, usually
    /// `include_str!` of a key file. With a key, a release must carry
    /// `checksums.txt.sig`, a raw 64-byte signature over the exact bytes of
    /// `checksums.txt`, or nothing is downloaded. `None` trusts the checksum
    /// file as served by GitHub, which only protects against corruption.
    pub publisher_key: Option<&'static str>,
    /// More publisher keys a release may be signed with, for rotating keys:
    /// ship the next key here, keep signing with `publisher_key`, then sign
    /// with the next key once installs trust it. A signature valid under any
    /// of these or `publisher_key` is accepted. Needs `publisher_key`.
    pub additional_publisher_keys: &'static [&'static str],
}

impl UpdateConfig {
    /// A configuration with no legacy names, no macOS identifiers and no
    /// publisher key.
    pub const fn new(
        repository: &'static str,
        app_name: &'static str,
        slug: &'static str,
        current_version: &'static str,
    ) -> Self {
        Self {
            repository,
            app_name,
            slug,
            current_version,
            legacy_names: &[],
            legacy_windows_installs: &[],
            macos: MacConfig {
                bundle_ids: &[],
                executable_names: &[],
                legacy_bundle_names: &[],
            },
            publisher_key: None,
            additional_publisher_keys: &[],
        }
    }

    /// Checks the values an app cannot get right by accident: the
    /// repository shape, the slug, the version, and the publisher key. Call
    /// it from a unit test in the app.
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::ensure;
        let valid_name = |name: &str| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        };
        ensure!(
            self.repository
                .split_once('/')
                .is_some_and(|(owner, name)| valid_name(owner) && valid_name(name)),
            "The repository must be owner/name"
        );
        for name in self.names() {
            ensure!(
                valid_name(name) && name == name.to_ascii_lowercase(),
                "Command names must be lowercase: {name}"
            );
        }
        ensure!(
            version::parse(self.current_version).is_some(),
            "The current version must be major.minor.patch"
        );
        if let Some(key) = self.publisher_key {
            signing::decode_key(key)?;
        }
        ensure!(
            self.publisher_key.is_some() || self.additional_publisher_keys.is_empty(),
            "Additional publisher keys need a publisher key"
        );
        for key in self.additional_publisher_keys {
            signing::decode_key(key)?;
        }
        Ok(())
    }

    /// The slug, then each legacy name.
    fn names(&self) -> impl Iterator<Item = &'static str> + Clone {
        std::iter::once(self.slug).chain(self.legacy_names.iter().copied())
    }

    fn user_agent(&self) -> String {
        format!("{}/{}", self.app_name, self.current_version)
    }
}

/// The macOS bundle's identity, checked before an update is accepted.
#[derive(Clone, Copy, Debug, Default)]
pub struct MacConfig {
    /// Accepted `CFBundleIdentifier` values, for example
    /// `["rocks.spotifast.Spotifast", "me.paolino.fastpotify"]`. An update is
    /// refused when the running bundle or the download has any other.
    pub bundle_ids: &'static [&'static str],
    /// Accepted `CFBundleExecutable` names. Empty means just the slug. A
    /// release may rename the executable only if the running version already
    /// lists the new name.
    pub executable_names: &'static [&'static str],
    /// Bundle names used before, for example `FastsApp.app`. A disk image
    /// may carry either `<app_name>.app` or one of these.
    pub legacy_bundle_names: &'static [&'static str],
}

/// Where an update stands, for the app's interface. The updater itself does
/// not use it; both apps kept this same state, so it lives here.
#[derive(Debug, Default)]
pub enum DownloadState {
    /// Nothing downloaded.
    #[default]
    Idle,
    /// Downloading: bytes received of the published size.
    Downloading {
        /// Bytes received so far.
        received: u64,
        /// The published size, or zero before it is known.
        total: u64,
    },
    /// Verified and staged, waiting for the user to restart.
    Ready(Box<Prepared>),
    /// The helper has been started; the app is about to quit.
    Installing,
    /// The download or handoff failed, with a message for the user.
    Failed(String),
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod compat;
#[cfg(test)]
mod testing;

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const ZAPFAST: UpdateConfig = UpdateConfig {
        legacy_names: &["fastsapp"],
        macos: MacConfig {
            bundle_ids: &["me.paolino.fastsapp"],
            executable_names: &[],
            legacy_bundle_names: &["FastsApp.app"],
        },
        publisher_key: Some(include_str!("../tests/fixtures/zapfast-public-key.hex")),
        ..UpdateConfig::new("crmne/zapfast", "ZapFast", "zapfast", "0.16.3")
    };

    #[test]
    fn a_config_checks_its_own_values() {
        ZAPFAST.validate().unwrap();
        for broken in [
            UpdateConfig {
                repository: "zapfast",
                ..ZAPFAST
            },
            UpdateConfig {
                slug: "ZapFast",
                ..ZAPFAST
            },
            UpdateConfig {
                current_version: "nightly",
                ..ZAPFAST
            },
            UpdateConfig {
                publisher_key: Some("c2256da7"),
                ..ZAPFAST
            },
            UpdateConfig {
                legacy_names: &["../escape"],
                ..ZAPFAST
            },
        ] {
            assert!(broken.validate().is_err(), "{broken:?}");
        }
    }

    #[test]
    fn the_user_agent_names_the_app_not_the_framework() {
        assert_eq!(ZAPFAST.user_agent(), "ZapFast/0.16.3");
    }
}
