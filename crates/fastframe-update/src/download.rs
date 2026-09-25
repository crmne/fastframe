//! Downloading and verifying a release into a staging folder.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use url::Url;

use crate::detect::{Installation, Kind, Platform};
use crate::host::Host;
use crate::release::{self, MANIFEST_LIMIT, Release};
use crate::stage::{self, Prepared, Staged};
use crate::transport::{Source, Transport, fetch};
use crate::{UpdateConfig, hex, signing, version};

const BINARY: &str = "application/octet-stream";

/// Everything a download reads from its surroundings.
pub(crate) struct Inputs<'a> {
    pub(crate) config: &'a UpdateConfig,
    pub(crate) transport: &'a dyn Transport,
    pub(crate) source: &'a Source,
    pub(crate) host: &'a dyn Host,
    pub(crate) platform: Platform,
    pub(crate) arch: &'a str,
}

/// Downloads `release` for `installation` and verifies it. Nothing is
/// written before the checksum file's publisher signature checks out, and
/// a failure removes the staging folder again.
pub(crate) fn download(
    inputs: &Inputs<'_>,
    installation: Installation,
    release: &Release,
    mut progress: impl FnMut(u64, u64),
) -> Result<Prepared> {
    let Inputs {
        config,
        transport,
        source,
        host,
        platform,
        arch,
    } = *inputs;
    ensure!(
        version::is_plain_release(&release.version),
        "Invalid release version"
    );
    let keys = config
        .publisher_key
        .into_iter()
        .chain(config.additional_publisher_keys.iter().copied())
        .map(signing::decode_key)
        .collect::<Result<Vec<_>>>()?;
    let key = keys.first().copied();
    let metadata = release::metadata(config, transport, source, &release.version)?;
    let target = release::target(platform, arch)?;
    let stem = release::stem(config, &release.version, target);
    let name = release::asset_name(&stem, installation.kind, platform);
    let package = metadata.asset(&name)?;
    let checksums = metadata.asset("checksums.txt")?;
    let signature = key
        .map(|_| metadata.asset("checksums.txt.sig"))
        .transpose()?;
    ensure!(
        checksums.size <= MANIFEST_LIMIT && signature.is_none_or(|signature| signature.size == 64),
        "Invalid signed update metadata size"
    );
    for asset in [Some(package), Some(checksums), signature]
        .into_iter()
        .flatten()
    {
        let url = Url::parse(&asset.browser_download_url)?;
        ensure!(
            source.owns_asset(config, &url, &release.version, &asset.name),
            "Update asset does not belong to this release"
        );
    }
    let mut manifest = Vec::new();
    fetch(
        transport,
        source,
        config,
        &checksums.browser_download_url,
        BINARY,
    )?
    .take(MANIFEST_LIMIT + 1)
    .read_to_end(&mut manifest)?;
    ensure!(
        manifest.len() as u64 == checksums.size,
        "Invalid update checksum download size"
    );
    if let (Some(_), Some(signature)) = (key, signature) {
        let mut bytes = Vec::new();
        fetch(
            transport,
            source,
            config,
            &signature.browser_download_url,
            BINARY,
        )?
        .take(65)
        .read_to_end(&mut bytes)?;
        // Do not parse a checksum, download a package or create staging
        // files until the manifest is authorized by the embedded key.
        // Any trusted key will do: during a key rotation releases are
        // signed with one while installs trust both.
        let mut result = signing::verify(&manifest, &bytes, &keys[0]);
        for other in &keys[1..] {
            if result.is_ok() {
                break;
            }
            result = signing::verify(&manifest, &bytes, other);
        }
        result?;
    }
    let expected = release::checksum(
        std::str::from_utf8(&manifest).context("Invalid update checksum text")?,
        &name,
    )?;
    let directory = stage::staging(config, &installation)?;
    let result = (|| -> Result<Staged> {
        let archive = directory.join(&name);
        let mut output = File::create(&archive)?;
        let mut response = fetch(
            transport,
            source,
            config,
            &package.browser_download_url,
            BINARY,
        )?;
        let mut hash = Sha256::new();
        let mut received = 0;
        let mut buffer = vec![0; 64 * 1024];
        loop {
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            received += count as u64;
            ensure!(
                received <= package.size,
                "Update download exceeds its published size"
            );
            hash.update(&buffer[..count]);
            output.write_all(&buffer[..count])?;
            if received % (1024 * 1024) < count as u64 || received == package.size {
                progress(received, package.size);
            }
        }
        output.sync_all()?;
        drop(output);
        ensure!(
            received == package.size,
            "The update download was interrupted"
        );
        ensure!(
            hex(&hash.finalize()) == expected,
            "The download couldn't be verified. Try downloading it again."
        );
        let payload = match installation.kind {
            Kind::MacBundle => {
                crate::macos::validate_download(
                    config,
                    host,
                    &archive,
                    &installation,
                    &release.version,
                )?;
                archive
            }
            Kind::WindowsInstaller => archive,
            Kind::Portable => {
                let executable = release::portable_executable(config, platform);
                let payload = directory.join(&executable);
                host.extract(&archive, &format!("{stem}/{executable}"), &payload)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&payload, fs::Permissions::from_mode(0o755))?;
                }
                check_version(config, host, &payload, &release.version)?;
                fs::remove_file(&archive)?;
                payload
            }
        };
        Ok(Staged {
            sha256: stage::hash(&payload)?,
            installation,
            directory: directory.clone(),
            payload,
            version: release.version.clone(),
        })
    })();
    match result {
        Ok(staged) => Ok(Prepared {
            staged,
            sample: false,
        }),
        Err(error) => {
            stage::discard(&directory);
            Err(error)
        }
    }
}

/// Runs `executable --version` and expects `<slug> <version>`, or a legacy
/// name in place of the slug.
pub(crate) fn check_version(
    config: &UpdateConfig,
    host: &dyn Host,
    executable: &Path,
    version: &str,
) -> Result<()> {
    let output = host.version_output(executable)?;
    ensure!(
        config
            .names()
            .any(|name| output == format!("{name} {version}")),
        "The downloaded app has the wrong version"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHost, FakeTransport, fixture_key};
    use crate::tests::ZAPFAST;
    use ring::signature::KeyPair;

    const VERSION: &str = "0.17.0";

    /// A release with a package, a checksum file and (optionally) its
    /// signature, served by a fake GitHub.
    struct Fixture {
        package: Vec<u8>,
        listed_digest: Option<String>,
        signed_manifest: Option<String>,
        signature: bool,
        published_size: Option<usize>,
        kind: Kind,
        platform: Platform,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                package: b"a verified package".to_vec(),
                listed_digest: None,
                signed_manifest: None,
                signature: true,
                published_size: None,
                kind: Kind::Portable,
                platform: Platform::Linux,
            }
        }
    }

    fn asset_url(name: &str) -> String {
        format!("https://github.com/crmne/zapfast/releases/download/v{VERSION}/{name}")
    }

    impl Fixture {
        fn name(&self) -> String {
            let target = release::target(self.platform, "x86_64").unwrap();
            release::asset_name(
                &release::stem(&ZAPFAST, VERSION, target),
                self.kind,
                self.platform,
            )
        }

        fn transport(&self) -> FakeTransport {
            let name = self.name();
            let digest = self
                .listed_digest
                .clone()
                .unwrap_or_else(|| hex(&Sha256::digest(&self.package)));
            let manifest = format!(
                "{digest}  {name}\n{}  checksums-other.txt\n",
                "0".repeat(64)
            );
            let signed = self
                .signed_manifest
                .clone()
                .unwrap_or_else(|| manifest.clone());
            let signature = fixture_key().sign(signed.as_bytes());
            let mut assets = vec![
                serde_json::json!({"name": name, "size": self.published_size.unwrap_or(self.package.len()), "browser_download_url": asset_url(&name)}),
                serde_json::json!({"name": "checksums.txt", "size": manifest.len(), "browser_download_url": asset_url("checksums.txt")}),
            ];
            if self.signature {
                assets.push(serde_json::json!({"name": "checksums.txt.sig", "size": 64, "browser_download_url": asset_url("checksums.txt.sig")}));
            }
            let metadata = serde_json::json!({"tag_name": format!("v{VERSION}"), "draft": false, "prerelease": false, "assets": assets});
            FakeTransport::default()
                .serve(
                    &format!("https://api.github.com/repos/crmne/zapfast/releases/tags/v{VERSION}"),
                    metadata.to_string().as_bytes(),
                )
                .redirect(
                    &asset_url(&name),
                    "https://release-assets.githubusercontent.com/package",
                )
                .serve(
                    "https://release-assets.githubusercontent.com/package",
                    &self.package,
                )
                .serve(&asset_url("checksums.txt"), manifest.as_bytes())
                .serve(&asset_url("checksums.txt.sig"), signature.as_ref())
        }
    }

    fn config() -> UpdateConfig {
        let key = crate::testing::fixture_key_hex();
        UpdateConfig {
            publisher_key: Some(key),
            ..ZAPFAST
        }
    }

    fn run(
        fixture: &Fixture,
        config: &UpdateConfig,
        host: &FakeHost,
        installation: Installation,
    ) -> Result<Prepared> {
        let transport = fixture.transport();
        download(
            &Inputs {
                config,
                transport: &transport,
                source: &Source::github(),
                host,
                platform: fixture.platform,
                arch: "x86_64",
            },
            installation,
            &Release {
                version: VERSION.into(),
                url: String::new(),
            },
            |_, _| {},
        )
    }

    fn portable(directory: &Path) -> Installation {
        let executable = directory.join("zapfast");
        fs::write(&executable, b"the running app").unwrap();
        Installation {
            executable,
            kind: Kind::Portable,
        }
    }

    fn untouched(directory: &Path) {
        assert_eq!(
            fs::read(directory.join("zapfast")).unwrap(),
            b"the running app"
        );
        assert_eq!(
            fs::read_dir(directory).unwrap().count(),
            1,
            "no staging folder is left behind"
        );
    }

    #[test]
    fn a_verified_portable_download_is_unpacked_and_checked() {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeHost::default()
            .with_archive_entry(
                "zapfast-v0.17.0-x86_64-unknown-linux-gnu/zapfast",
                b"new executable",
            )
            .with_version_output("zapfast 0.17.0");
        let prepared = run(
            &Fixture::default(),
            &config(),
            &host,
            portable(directory.path()),
        )
        .unwrap();
        let staged = &prepared.staged;
        assert_eq!(prepared.version(), "0.17.0");
        assert_eq!(staged.payload, staged.directory.join("zapfast"));
        assert_eq!(fs::read(&staged.payload).unwrap(), b"new executable");
        assert_eq!(staged.sha256, hex(&Sha256::digest(b"new executable")));
        assert!(
            !staged
                .directory
                .join("zapfast-v0.17.0-x86_64-unknown-linux-gnu.tar.gz")
                .exists(),
            "the archive is removed after unpacking"
        );
        assert_eq!(host.version_probes(), std::slice::from_ref(&staged.payload));
        assert_eq!(staged.directory.parent(), Some(directory.path()));
    }

    #[test]
    fn a_wrong_version_answer_discards_the_download() {
        let directory = tempfile::tempdir().unwrap();
        for answer in ["zapfast 0.16.3", "spotifast 0.17.0", "zapfast 0.17.0 extra"] {
            let host = FakeHost::default()
                .with_archive_entry(
                    "zapfast-v0.17.0-x86_64-unknown-linux-gnu/zapfast",
                    b"new executable",
                )
                .with_version_output(answer);
            let error = run(
                &Fixture::default(),
                &config(),
                &host,
                portable(directory.path()),
            )
            .unwrap_err();
            assert!(error.to_string().contains("wrong version"), "{error:#}");
            untouched(directory.path());
        }
        // A legacy name is a valid answer.
        let host = FakeHost::default()
            .with_archive_entry(
                "zapfast-v0.17.0-x86_64-unknown-linux-gnu/zapfast",
                b"new executable",
            )
            .with_version_output("fastsapp 0.17.0");
        run(
            &Fixture::default(),
            &config(),
            &host,
            portable(directory.path()),
        )
        .unwrap();
    }

    #[test]
    fn checksum_and_signature_failures_leave_nothing_behind() {
        let forged = format!(
            "{}  zapfast-v0.17.0-x86_64-unknown-linux-gnu.tar.gz\n",
            "0".repeat(64)
        );
        for (fixture, message) in [
            (
                Fixture {
                    listed_digest: Some("0".repeat(64)),
                    ..Fixture::default()
                },
                "verified",
            ),
            (
                Fixture {
                    published_size: Some(19),
                    ..Fixture::default()
                },
                "interrupted",
            ),
            (
                Fixture {
                    published_size: Some(4),
                    ..Fixture::default()
                },
                "exceeds",
            ),
            (
                Fixture {
                    signed_manifest: Some(forged),
                    ..Fixture::default()
                },
                "signature",
            ),
            (
                Fixture {
                    signature: false,
                    ..Fixture::default()
                },
                "checksums.txt.sig",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let host = FakeHost::default();
            let transport = fixture.transport();
            let error = run(&fixture, &config(), &host, portable(directory.path())).unwrap_err();
            assert!(error.to_string().contains(message), "{message}: {error:#}");
            untouched(directory.path());
            if message == "signature" || message == "checksums.txt.sig" {
                assert!(
                    !transport
                        .requested()
                        .iter()
                        .any(|url| url.contains("release-assets")),
                    "an unauthorized manifest never leads to a package download"
                );
            }
        }
    }

    #[test]
    fn a_signature_from_an_additional_key_is_accepted() {
        let directory = tempfile::tempdir().unwrap();
        let other = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let other = hex(other.public_key().as_ref());
        let fixture = crate::testing::fixture_key_hex();
        let config = UpdateConfig {
            publisher_key: Some(Box::leak(other.into_boxed_str())),
            additional_publisher_keys: Box::leak(vec![fixture].into_boxed_slice()),
            ..ZAPFAST
        };
        let host = FakeHost::default()
            .with_archive_entry(
                "zapfast-v0.17.0-x86_64-unknown-linux-gnu/zapfast",
                b"new executable",
            )
            .with_version_output("zapfast 0.17.0");
        let prepared = run(
            &Fixture::default(),
            &config,
            &host,
            portable(directory.path()),
        )
        .expect("a release signed with a key in rotation is accepted");
        assert_eq!(prepared.version(), "0.17.0");
    }

    #[test]
    fn a_signature_from_another_key_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let other = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let key = hex(other.public_key().as_ref());
        let config = UpdateConfig {
            publisher_key: Some(Box::leak(key.into_boxed_str())),
            ..ZAPFAST
        };
        let error = run(
            &Fixture::default(),
            &config,
            &FakeHost::default(),
            portable(directory.path()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("signature"), "{error:#}");
        untouched(directory.path());
    }

    #[test]
    fn without_a_publisher_key_the_checksum_file_alone_is_trusted() {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeHost::default()
            .with_archive_entry(
                "zapfast-v0.17.0-x86_64-unknown-linux-gnu/zapfast",
                b"new executable",
            )
            .with_version_output("zapfast 0.17.0");
        let unsigned = UpdateConfig {
            publisher_key: None,
            ..ZAPFAST
        };
        let fixture = Fixture {
            signature: false,
            ..Fixture::default()
        };
        run(&fixture, &unsigned, &host, portable(directory.path())).unwrap();
        let fixture = Fixture {
            listed_digest: Some("0".repeat(64)),
            signature: false,
            ..Fixture::default()
        };
        let other = tempfile::tempdir().unwrap();
        assert!(run(&fixture, &unsigned, &host, portable(other.path())).is_err());
        untouched(other.path());
    }

    #[test]
    fn the_windows_installer_is_staged_as_downloaded() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("zapfast.exe");
        fs::write(&executable, b"the running app").unwrap();
        let fixture = Fixture {
            kind: Kind::WindowsInstaller,
            platform: Platform::Windows,
            package: b"setup program".to_vec(),
            ..Fixture::default()
        };
        let host = FakeHost::default();
        let prepared = run(
            &fixture,
            &config(),
            &host,
            Installation {
                executable,
                kind: Kind::WindowsInstaller,
            },
        )
        .unwrap();
        assert!(
            prepared
                .staged
                .payload
                .ends_with("zapfast-v0.17.0-x86_64-pc-windows-msvc-setup.exe")
        );
        assert_eq!(
            fs::read(&prepared.staged.payload).unwrap(),
            b"setup program"
        );
        assert!(host.version_probes().is_empty());
    }

    #[test]
    fn assets_outside_the_release_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = Fixture::default();
        let name = fixture.name();
        let metadata = serde_json::json!({"tag_name": "v0.17.0", "assets": [
            {"name": name, "size": 18, "browser_download_url": "https://github.com/crmne/other/releases/download/v0.17.0/x"},
            {"name": "checksums.txt", "size": 10, "browser_download_url": asset_url("checksums.txt")},
            {"name": "checksums.txt.sig", "size": 64, "browser_download_url": asset_url("checksums.txt.sig")},
        ]});
        let transport = FakeTransport::default().serve(
            "https://api.github.com/repos/crmne/zapfast/releases/tags/v0.17.0",
            metadata.to_string().as_bytes(),
        );
        let error = download(
            &Inputs {
                config: &config(),
                transport: &transport,
                source: &Source::github(),
                host: &FakeHost::default(),
                platform: Platform::Linux,
                arch: "x86_64",
            },
            portable(directory.path()),
            &Release {
                version: VERSION.into(),
                url: String::new(),
            },
            |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not belong"), "{error:#}");
        untouched(directory.path());
    }

    #[test]
    fn a_changed_or_prerelease_tag_is_refused() {
        for metadata in [
            serde_json::json!({"tag_name": "v0.17.1", "assets": []}),
            serde_json::json!({"tag_name": "v0.17.0", "prerelease": true, "assets": []}),
            serde_json::json!({"tag_name": "v0.17.0", "draft": true, "assets": []}),
        ] {
            let transport = FakeTransport::default().serve(
                "https://api.github.com/repos/crmne/zapfast/releases/tags/v0.17.0",
                metadata.to_string().as_bytes(),
            );
            let directory = tempfile::tempdir().unwrap();
            let error = download(
                &Inputs {
                    config: &config(),
                    transport: &transport,
                    source: &Source::github(),
                    host: &FakeHost::default(),
                    platform: Platform::Linux,
                    arch: "x86_64",
                },
                portable(directory.path()),
                &Release {
                    version: VERSION.into(),
                    url: String::new(),
                },
                |_, _| {},
            )
            .unwrap_err();
            assert!(error.to_string().contains("changed"), "{error:#}");
        }
    }

    #[test]
    fn invalid_versions_never_reach_a_request() {
        let transport = FakeTransport::default();
        let directory = tempfile::tempdir().unwrap();
        for version in ["0.17.0-rc1", "../0.17.0", "0.17"] {
            assert!(
                download(
                    &Inputs {
                        config: &config(),
                        transport: &transport,
                        source: &Source::github(),
                        host: &FakeHost::default(),
                        platform: Platform::Linux,
                        arch: "x86_64",
                    },
                    portable(directory.path()),
                    &Release {
                        version: version.into(),
                        url: String::new(),
                    },
                    |_, _| {},
                )
                .is_err()
            );
        }
        assert!(transport.requested().is_empty());
    }

    #[test]
    fn progress_is_reported_up_to_the_published_size() {
        let directory = tempfile::tempdir().unwrap();
        let host = FakeHost::default()
            .with_archive_entry(
                "zapfast-v0.17.0-x86_64-unknown-linux-gnu/zapfast",
                b"new executable",
            )
            .with_version_output("zapfast 0.17.0");
        let fixture = Fixture {
            package: vec![7; 3 * 1024 * 1024 + 5],
            ..Fixture::default()
        };
        let transport = fixture.transport();
        let mut reports = Vec::new();
        download(
            &Inputs {
                config: &config(),
                transport: &transport,
                source: &Source::github(),
                host: &host,
                platform: Platform::Linux,
                arch: "x86_64",
            },
            portable(directory.path()),
            &Release {
                version: VERSION.into(),
                url: String::new(),
            },
            |received, total| reports.push((received, total)),
        )
        .unwrap();
        let total = fixture.package.len() as u64;
        assert!(reports.len() >= 4, "{reports:?}");
        assert_eq!(reports.last(), Some(&(total, total)));
        assert!(reports.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }
}
