//! The app's handle on the updater.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::detect::{self, Installation, Platform, Unsupported};
use crate::download::{self, Inputs};
use crate::host::{Host, OsHost};
use crate::release::{self, Release};
use crate::stage::Prepared;
use crate::transport::{Source, Transport};
use crate::{UpdateConfig, helper};

/// Checks for, downloads and installs updates. Cheap to clone; every call
/// blocks, so run them off the interface thread.
#[derive(Clone)]
pub struct Updater {
    config: UpdateConfig,
    transport: Arc<dyn Transport>,
    source: Source,
    host: Arc<dyn Host>,
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Updater")
            .field("config", &self.config)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl Updater {
    /// An updater for GitHub releases that uses the app's HTTP client.
    pub fn new(config: UpdateConfig, transport: impl Transport + 'static) -> Self {
        Self {
            config,
            transport: Arc::new(transport),
            source: Source::github(),
            host: Arc::new(OsHost),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_host(mut self, host: impl Host + 'static) -> Self {
        self.host = Arc::new(host);
        self
    }

    /// Takes releases from `source` instead of GitHub.
    pub fn with_source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    /// The configuration.
    pub fn config(&self) -> &UpdateConfig {
        &self.config
    }

    /// Where releases come from.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The newest release, when it is newer than
    /// [`UpdateConfig::current_version`].
    pub fn check(&self) -> Result<Option<Release>> {
        release::newer(&self.config, self.transport.as_ref(), &self.source)
    }

    /// How this copy was installed, or why it cannot update itself. Reads
    /// files next to the executable and, on Linux, asks dpkg, rpm and pacman
    /// whether they own it.
    pub fn installation(&self) -> Result<Installation, Unsupported> {
        let executable = self
            .host
            .current_exe()
            .map_err(|error| Unsupported::Unavailable(error.to_string()))?;
        self.installation_at(&executable)
    }

    /// How the executable at `executable` (canonicalized) was installed, by
    /// the same rules as [`Updater::installation`]. For packaging checks and
    /// diagnostics; updates only ever replace the running app.
    pub fn installation_at(&self, executable: &Path) -> Result<Installation, Unsupported> {
        detect::detect(
            &self.config,
            self.host.as_ref(),
            Platform::current(),
            executable,
        )
    }

    /// Downloads and verifies `release` into a new staging folder beside
    /// the app. `progress` receives bytes received and the published size.
    pub fn download(&self, release: &Release, progress: impl FnMut(u64, u64)) -> Result<Prepared> {
        let installation = self.installation()?;
        download::download(
            &Inputs {
                config: &self.config,
                transport: self.transport.as_ref(),
                source: &self.source,
                host: self.host.as_ref(),
                platform: Platform::current(),
                arch: std::env::consts::ARCH,
            },
            installation,
            release,
            progress,
        )
    }

    /// Starts the helper and returns once it is watching this process. Quit
    /// the app right after: the helper waits one minute for it to exit, then
    /// installs and relaunches it with `arguments` (for example
    /// `--verbose`) followed by the receipt flag.
    pub fn handoff(&self, prepared: Prepared, arguments: Vec<String>) -> Result<()> {
        helper::handoff(&self.config, self.host.as_ref(), prepared, arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Kind;
    use crate::testing::{FakeHost, FakeTransport};
    use crate::tests::ZAPFAST;

    #[test]
    fn the_updater_checks_through_the_apps_transport_and_source() {
        let transport = FakeTransport::default().serve(
            "http://127.0.0.1:9/latest.json",
            br#"{"tag_name":"v0.17.0","html_url":"http://127.0.0.1:9/notes"}"#,
        );
        let updater = Updater::new(ZAPFAST, transport)
            .with_source(Source::local("http://127.0.0.1:9").unwrap());
        assert!(!updater.source().is_github());
        assert_eq!(updater.config().slug, "zapfast");
        assert_eq!(
            updater.check().unwrap(),
            Some(Release {
                version: "0.17.0".into(),
                url: "http://127.0.0.1:9/notes".into()
            })
        );
    }

    #[test]
    fn the_updater_inspects_its_own_executable() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join(if cfg!(windows) {
            "zapfast.exe"
        } else {
            "zapfast"
        });
        std::fs::write(&executable, b"app").unwrap();
        let updater = Updater::new(ZAPFAST, FakeTransport::default())
            .with_host(FakeHost::default().with_current_exe(&executable));
        if cfg!(any(target_os = "linux", windows)) {
            assert_eq!(updater.installation(), Err(Unsupported::NotPortable));
            std::fs::write(
                directory.path().join("zapfast-portable.txt"),
                "zapfast-portable-v1\n",
            )
            .unwrap();
            assert_eq!(updater.installation().unwrap().kind, Kind::Portable);
        }
        let without =
            Updater::new(ZAPFAST, FakeTransport::default()).with_host(FakeHost::default());
        assert!(matches!(
            without.installation(),
            Err(Unsupported::Unavailable(_))
        ));
    }

    #[test]
    fn a_download_for_an_unsupported_copy_touches_nothing() {
        let transport = FakeTransport::default();
        let updater = Updater::new(ZAPFAST, transport).with_host(FakeHost::default());
        let release = Release {
            version: "0.17.0".into(),
            url: String::new(),
        };
        assert!(updater.download(&release, |_, _| {}).is_err());
    }
}
