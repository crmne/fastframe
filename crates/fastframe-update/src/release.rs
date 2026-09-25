//! GitHub release listings and the names of release assets.

use std::io::Read;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::detect::{Kind, Platform, Unsupported};
use crate::stage::LIMIT;
use crate::transport::{Source, Transport, fetch};
use crate::{UpdateConfig, version};

/// A newer release than the running app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The version number, without a leading `v`.
    pub version: String,
    /// The release page, with every download and the release notes.
    pub url: String,
}

const JSON: &str = "application/vnd.github+json";
/// The largest release listing or checksum file accepted.
pub(crate) const MANIFEST_LIMIT: u64 = 1024 * 1024;

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    html_url: String,
}

/// The newest release, when it is newer than the running app.
pub(crate) fn newer(
    config: &UpdateConfig,
    transport: &dyn Transport,
    source: &Source,
) -> Result<Option<Release>> {
    let mut body = Vec::new();
    fetch(transport, source, config, &source.latest(config), JSON)?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut body)
        .context("Could not read the release listing")?;
    ensure!(
        body.len() as u64 <= MANIFEST_LIMIT,
        "The release listing is too large"
    );
    let latest: Latest = serde_json::from_slice(&body).context("Unexpected release listing")?;
    let version = latest.tag_name.trim_start_matches('v').to_owned();
    Ok(
        version::is_newer(&version, config.current_version).then_some(Release {
            version,
            url: latest.html_url,
        }),
    )
}

#[derive(Deserialize)]
pub(crate) struct Metadata {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) size: u64,
}

/// The release's metadata, checked to be the published stable release of
/// `version`.
pub(crate) fn metadata(
    config: &UpdateConfig,
    transport: &dyn Transport,
    source: &Source,
    version: &str,
) -> Result<Metadata> {
    let mut body = Vec::new();
    fetch(
        transport,
        source,
        config,
        &source.release(config, version),
        JSON,
    )?
    .take(MANIFEST_LIMIT + 1)
    .read_to_end(&mut body)?;
    ensure!(
        body.len() as u64 <= MANIFEST_LIMIT,
        "The release metadata is too large"
    );
    let metadata: Metadata =
        serde_json::from_slice(&body).context("Unexpected release metadata")?;
    ensure!(
        !metadata.draft && !metadata.prerelease && metadata.tag_name == format!("v{version}"),
        "The release changed. Check for updates again."
    );
    Ok(metadata)
}

impl Metadata {
    /// The one asset called `name`, with a plausible size.
    pub(crate) fn asset(&self, name: &str) -> Result<&Asset> {
        let matches: Vec<_> = self
            .assets
            .iter()
            .filter(|asset| asset.name == name)
            .collect();
        ensure!(
            matches.len() == 1,
            "The release has no unique {name} download"
        );
        let asset = matches[0];
        ensure!(
            asset.size > 0 && asset.size <= LIMIT,
            "Invalid update download size"
        );
        Ok(asset)
    }
}

/// The target part of release asset names for this operating system and
/// processor.
pub(crate) fn target(platform: Platform, arch: &str) -> Result<&'static str, Unsupported> {
    Ok(match (platform, arch) {
        (Platform::Windows, "x86_64") => "x86_64-pc-windows-msvc",
        (Platform::Windows, "aarch64") => "aarch64-pc-windows-msvc",
        (Platform::Linux, "x86_64") => "x86_64-unknown-linux-gnu",
        (Platform::Linux, "aarch64") => "aarch64-unknown-linux-gnu",
        (Platform::MacOs, "aarch64" | "x86_64") => "macos-universal",
        _ => return Err(Unsupported::Platform),
    })
}

/// `<slug>-v<version>-<target>`: the asset name without its extension, and
/// the folder inside a portable archive.
pub(crate) fn stem(config: &UpdateConfig, version: &str, target: &str) -> String {
    format!("{}-v{version}-{target}", config.slug)
}

/// The asset to download for this kind of installation.
pub(crate) fn asset_name(stem: &str, kind: Kind, platform: Platform) -> String {
    match kind {
        Kind::MacBundle => format!("{stem}.dmg"),
        Kind::WindowsInstaller => format!("{stem}-setup.exe"),
        Kind::Portable if platform == Platform::Windows => format!("{stem}.zip"),
        Kind::Portable => format!("{stem}.tar.gz"),
    }
}

/// The executable inside a portable archive, and its name once unpacked.
pub(crate) fn portable_executable(config: &UpdateConfig, platform: Platform) -> String {
    if platform == Platform::Windows {
        format!("{}.exe", config.slug)
    } else {
        config.slug.to_owned()
    }
}

/// The digest `checksums.txt` lists for exactly `name`: one line, 64
/// hexadecimal digits, in `sha256sum` format (`*` binary markers allowed).
pub(crate) fn checksum(text: &str, name: &str) -> Result<String> {
    let mut found = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if let (Some(digest), Some(file), None) = (fields.next(), fields.next(), fields.next())
            && file.trim_start_matches('*') == name
        {
            ensure!(found.is_none(), "Duplicate checksum for the update");
            ensure!(
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid update checksum"
            );
            found = Some(digest.to_ascii_lowercase());
        }
    }
    found.context("The release is missing the update checksum")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeTransport;
    use crate::tests::ZAPFAST;

    fn listing(tag: &str) -> String {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/crmne/zapfast/releases/tag/{tag}"),
        })
        .to_string()
    }

    const LATEST: &str = "https://api.github.com/repos/crmne/zapfast/releases/latest";

    #[test]
    fn a_newer_release_is_announced_against_the_apps_version() {
        let transport = FakeTransport::default().serve(LATEST, listing("v0.17.0").as_bytes());
        assert_eq!(
            newer(&ZAPFAST, &transport, &Source::github()).unwrap(),
            Some(Release {
                version: "0.17.0".into(),
                url: "https://github.com/crmne/zapfast/releases/tag/v0.17.0".into(),
            })
        );
        assert_eq!(transport.accepts(), ["application/vnd.github+json"]);
        for tag in ["v0.16.3", "v0.16.0", "v0.18.0-rc1", "nightly"] {
            let transport = FakeTransport::default().serve(LATEST, listing(tag).as_bytes());
            assert_eq!(
                newer(&ZAPFAST, &transport, &Source::github()).unwrap(),
                None,
                "{tag}"
            );
        }
        let older_app = UpdateConfig {
            current_version: "0.17.0-rc1",
            ..ZAPFAST
        };
        let transport = FakeTransport::default().serve(LATEST, listing("v0.17.0").as_bytes());
        assert!(
            newer(&older_app, &transport, &Source::github())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_broken_listing_is_an_error_not_an_update() {
        for body in [&b"not json"[..], b"{}", &vec![b' '; 2 * 1024 * 1024]] {
            let transport = FakeTransport::default().serve(LATEST, body);
            assert!(newer(&ZAPFAST, &transport, &Source::github()).is_err());
        }
    }

    #[test]
    fn asset_names_follow_the_release_contract() {
        let target = target(Platform::Linux, "x86_64").unwrap();
        let stem = stem(&ZAPFAST, "0.17.0", target);
        assert_eq!(stem, "zapfast-v0.17.0-x86_64-unknown-linux-gnu");
        assert_eq!(
            asset_name(&stem, Kind::Portable, Platform::Linux),
            "zapfast-v0.17.0-x86_64-unknown-linux-gnu.tar.gz"
        );
        let stem = super::stem(
            &ZAPFAST,
            "0.17.0",
            super::target(Platform::Windows, "aarch64").unwrap(),
        );
        assert_eq!(
            asset_name(&stem, Kind::Portable, Platform::Windows),
            "zapfast-v0.17.0-aarch64-pc-windows-msvc.zip"
        );
        assert_eq!(
            asset_name(&stem, Kind::WindowsInstaller, Platform::Windows),
            "zapfast-v0.17.0-aarch64-pc-windows-msvc-setup.exe"
        );
        let stem = super::stem(
            &ZAPFAST,
            "0.17.0",
            super::target(Platform::MacOs, "x86_64").unwrap(),
        );
        assert_eq!(
            asset_name(&stem, Kind::MacBundle, Platform::MacOs),
            "zapfast-v0.17.0-macos-universal.dmg"
        );
        assert_eq!(
            portable_executable(&ZAPFAST, Platform::Windows),
            "zapfast.exe"
        );
        assert_eq!(portable_executable(&ZAPFAST, Platform::Linux), "zapfast");
        assert_eq!(
            super::target(Platform::Linux, "riscv64"),
            Err(Unsupported::Platform)
        );
        assert_eq!(
            super::target(Platform::Other, "x86_64"),
            Err(Unsupported::Platform)
        );
    }

    #[test]
    fn checksums_must_be_unique_valid_and_for_the_exact_asset() {
        let digest = "a".repeat(64);
        let valid = format!("{digest}  app.zip\n");
        assert_eq!(checksum(&valid, "app.zip").unwrap(), digest);
        assert_eq!(
            checksum(&format!("{digest} *app.zip\n"), "app.zip").unwrap(),
            digest
        );
        assert_eq!(
            checksum(&format!("{}  app.zip\n", "A".repeat(64)), "app.zip").unwrap(),
            digest
        );
        assert!(checksum(&valid, "other.zip").is_err());
        assert!(checksum(&(valid.clone() + &valid), "app.zip").is_err());
        assert!(checksum("invalid app.zip", "app.zip").is_err());
        assert!(checksum(&format!("{}  app.zip\n", "g".repeat(64)), "app.zip").is_err());
        // A correctly signed old manifest cannot authorize another version:
        // the version is part of the exact asset name.
        assert!(
            checksum(
                &format!("{digest} zapfast-v0.8.0-test.zip\n"),
                "zapfast-v0.9.0-test.zip"
            )
            .is_err()
        );
    }

    #[test]
    fn the_native_packages_checksum_fixture_parses() {
        let text = include_str!("../tests/fixtures/zapfast-0.16.3-checksums.txt");
        let digest = checksum(text, "zapfast-v0.16.3-x86_64-unknown-linux-gnu.tar.gz").unwrap();
        assert_eq!(digest.len(), 64);
        assert!(checksum(text, "zapfast-v0.16.3-macos-universal.dmg").is_ok());
    }

    #[test]
    fn spotifasts_published_checksums_name_every_update_asset() {
        const SPOTIFAST: UpdateConfig =
            UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.10.0");
        let text = include_str!("../tests/fixtures/spotifast-0.10.1-checksums.txt");
        for (platform, arch, kind) in [
            (Platform::Linux, "x86_64", Kind::Portable),
            (Platform::Linux, "aarch64", Kind::Portable),
            (Platform::Windows, "x86_64", Kind::Portable),
            (Platform::Windows, "aarch64", Kind::WindowsInstaller),
            (Platform::MacOs, "aarch64", Kind::MacBundle),
        ] {
            let stem = stem(&SPOTIFAST, "0.10.1", target(platform, arch).unwrap());
            let name = asset_name(&stem, kind, platform);
            assert!(checksum(text, &name).is_ok(), "{name}");
        }
    }
}
