//! The contract with versions released before this crate existed.
//!
//! The fixtures in `tests/fixtures/*.json` were written by the `Handoff`
//! structs of ZapFast 0.16.3 and Spotifast 0.10.1, copied verbatim and
//! serialized with serde_json the way those apps do (`to_writer` in
//! `handoff`, `to_vec` in Spotifast's `write_receipt`). An installed old
//! version runs the helper and the new version reads its receipt, so both
//! directions must keep working.

use std::fs;
use std::path::Path;

use crate::UpdateConfig;
use crate::detect::Kind;
use crate::stage::{self, Handoff, STARTED};
use crate::testing::FakeHost;
use crate::tests::ZAPFAST;

const SPOTIFAST: UpdateConfig = UpdateConfig {
    legacy_names: &["fastpotify"],
    macos: crate::MacConfig {
        bundle_ids: &["rocks.spotifast.Spotifast", "me.paolino.fastpotify"],
        executable_names: &["fastpotify", "Spotifast"],
        legacy_bundle_names: &["Fastpotify.app"],
    },
    ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", "0.11.0")
};

const FIXTURES: [(&str, &str); 6] = [
    (
        "zapfast-0.16.3-portable-linux.json",
        include_str!("../tests/fixtures/zapfast-0.16.3-portable-linux.json"),
    ),
    (
        "zapfast-0.16.3-installer-windows.json",
        include_str!("../tests/fixtures/zapfast-0.16.3-installer-windows.json"),
    ),
    (
        "zapfast-0.16.3-bundle-macos.json",
        include_str!("../tests/fixtures/zapfast-0.16.3-bundle-macos.json"),
    ),
    (
        "spotifast-0.10.1-portable-linux.json",
        include_str!("../tests/fixtures/spotifast-0.10.1-portable-linux.json"),
    ),
    (
        "spotifast-0.10.1-bundle-macos-renamed.json",
        include_str!("../tests/fixtures/spotifast-0.10.1-bundle-macos-renamed.json"),
    ),
    (
        "fastpotify-0.8.0-portable-linux.json",
        include_str!("../tests/fixtures/fastpotify-0.8.0-portable-linux.json"),
    ),
];

#[test]
fn old_jobs_read_and_write_back_byte_for_byte() {
    for (name, text) in FIXTURES {
        let handoff: Handoff =
            serde_json::from_str(text).unwrap_or_else(|error| panic!("{name}: {error}"));
        // What this crate's handoff writes.
        let directory = tempfile::tempdir().unwrap();
        let job = directory.path().join("handoff.json");
        stage::create_handoff(&job, &handoff).unwrap();
        assert_eq!(fs::read_to_string(&job).unwrap(), text, "{name}");
        // What this crate's helper writes after a renamed executable.
        stage::rewrite_handoff(&job, &handoff).unwrap();
        assert_eq!(fs::read_to_string(&job).unwrap(), text, "{name}");
        assert_eq!(stage::read_handoff(&job).unwrap(), handoff, "{name}");
    }
}

#[test]
fn old_jobs_carry_the_expected_values() {
    let zapfast: Handoff = serde_json::from_str(FIXTURES[0].1).unwrap();
    assert_eq!(zapfast.prepared.installation.kind, Kind::Portable);
    assert_eq!(zapfast.prepared.version, "0.17.0");
    assert_eq!(zapfast.parent, 48213);
    assert_eq!(zapfast.arguments, ["--verbose"]);
    let windows: Handoff = serde_json::from_str(FIXTURES[1].1).unwrap();
    assert_eq!(windows.prepared.installation.kind, Kind::WindowsInstaller);
    assert!(windows.arguments.is_empty());
    let macos: Handoff = serde_json::from_str(FIXTURES[2].1).unwrap();
    assert_eq!(macos.prepared.installation.kind, Kind::MacBundle);
    let renamed: Handoff = serde_json::from_str(FIXTURES[4].1).unwrap();
    assert!(
        renamed
            .prepared
            .installation
            .executable
            .ends_with("Spotifast.app/Contents/MacOS/Spotifast")
    );
}

#[test]
fn staging_names_from_every_old_version_are_recognized() {
    for (name, text) in FIXTURES {
        let handoff: Handoff = serde_json::from_str(text).unwrap();
        let config = if name.starts_with("zapfast") {
            ZAPFAST
        } else {
            SPOTIFAST
        };
        let directory = handoff
            .prepared
            .directory
            .to_string_lossy()
            .replace('\\', "/");
        let folder = directory.rsplit('/').next().unwrap();
        assert!(stage::is_staging_name(&config, folder), "{name}: {folder}");
    }
}

/// Puts a fixture's installation under `root` and returns the job path.
fn install(
    text: &str,
    old_root: &str,
    root: &Path,
    executable_name: &str,
    payload_name: &str,
) -> std::path::PathBuf {
    let escaped = serde_json::to_string(root.to_str().unwrap()).unwrap();
    let escaped = escaped.trim_matches('"');
    let mut text = text.replace(old_root, escaped);
    if cfg!(windows) {
        // Verbatim (`\\?\`) paths only accept backslashes, as Windows
        // writes them.
        text = text.replace('/', "\\\\");
    }
    let handoff: Handoff = serde_json::from_str(&text).unwrap();
    let executable = executable_name
        .split('/')
        .fold(root.to_owned(), |path, part| path.join(part));
    fs::write(executable, b"new executable").unwrap();
    fs::create_dir(&handoff.prepared.directory).unwrap();
    fs::write(handoff.prepared.directory.join(payload_name), b"payload").unwrap();
    let job = handoff.prepared.directory.join("handoff.json");
    fs::write(&job, text).unwrap();
    job
}

#[test]
fn the_new_app_acknowledges_receipts_from_old_helpers() {
    for (config, fixture, old_root, executable, payload) in [
        (
            UpdateConfig {
                current_version: "0.17.0",
                ..ZAPFAST
            },
            FIXTURES[0].1,
            "/home/user/Apps/ZapFast",
            "zapfast",
            "zapfast",
        ),
        (
            SPOTIFAST,
            FIXTURES[3].1,
            "/home/user/Apps/Spotifast",
            "spotifast",
            "spotifast",
        ),
        // A Fastpotify-era helper staged under its old name and replaced an
        // executable still called fastpotify.
        (
            SPOTIFAST,
            FIXTURES[5].1,
            "/home/user/Apps/Spotifast",
            "fastpotify",
            "spotifast",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let job = install(fixture, old_root, &root, executable, payload);
        let host = FakeHost::default().with_current_exe(&root.join(executable));
        crate::helper::acknowledge(&config, &host, &job).unwrap();
        assert_eq!(
            fs::read_to_string(job.parent().unwrap().join(STARTED)).unwrap(),
            config.current_version,
            "the old helper waits for this file"
        );
    }
}

#[test]
fn a_receipt_for_another_version_or_copy_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let job = install(
        FIXTURES[0].1,
        "/home/user/Apps/ZapFast",
        &root,
        "zapfast",
        "zapfast",
    );
    let host = FakeHost::default().with_current_exe(&root.join("zapfast"));
    // Still the old version: the replacement did not take.
    assert!(crate::helper::acknowledge(&ZAPFAST, &host, &job).is_err());
    // Spotifast reading ZapFast's receipt: the staging name is not its own.
    assert!(
        crate::helper::acknowledge(
            &UpdateConfig {
                current_version: "0.17.0",
                ..SPOTIFAST
            },
            &host,
            &job
        )
        .is_err()
    );
    assert!(!job.parent().unwrap().join(STARTED).exists());
}

#[cfg(windows)]
#[test]
fn the_new_app_acknowledges_an_old_windows_installer_receipt() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let job = install(
        FIXTURES[1].1,
        r"\\\\?\\C:\\Users\\user\\AppData\\Local\\Programs\\ZapFast",
        &root,
        "zapfast.exe",
        "zapfast-v0.17.0-x86_64-pc-windows-msvc-setup.exe",
    );
    let host = FakeHost::default().with_current_exe(&root.join("zapfast.exe"));
    let config = UpdateConfig {
        current_version: "0.17.0",
        ..ZAPFAST
    };
    crate::helper::acknowledge(&config, &host, &job).unwrap();
}

#[test]
fn macos_receipts_are_only_accepted_on_macos() {
    for (config, fixture, old_root, bundle, executable) in [
        (
            UpdateConfig {
                current_version: "0.17.0",
                ..ZAPFAST
            },
            FIXTURES[2].1,
            "/Applications",
            "ZapFast.app",
            "zapfast",
        ),
        // Spotifast's helper rewrote the receipt for the renamed executable.
        (
            SPOTIFAST,
            FIXTURES[4].1,
            "/Applications",
            "Spotifast.app",
            "Spotifast",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let macos = root.join(bundle).join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        let job = install(
            fixture,
            old_root,
            &root,
            &format!("{bundle}/Contents/MacOS/{executable}"),
            "update.dmg",
        );
        let host = FakeHost::default().with_current_exe(&macos.join(executable));
        let result = crate::helper::acknowledge(&config, &host, &job);
        if cfg!(target_os = "macos") {
            result.unwrap();
        } else {
            assert!(result.is_err());
        }
    }
}
