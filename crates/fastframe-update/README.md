# fastframe-update

Self-update from GitHub releases for desktop apps: check for a newer release,
decide whether this copy may replace itself, download and verify the update,
and hand it to a helper that installs it, relaunches the app and rolls back
if the new version does not start.

Extracted from ZapFast (0.16) and Spotifast (0.10), whose updaters had
drifted apart. It keeps ZapFast's publisher signatures and whole-bundle macOS
helper, and Spotifast's legacy names, renamed macOS executables and helper
log.

## Using it

```rust
use fastframe_update::{MacConfig, ReqwestTransport, UpdateConfig, Updater};

pub const UPDATES: UpdateConfig = UpdateConfig {
    legacy_names: &["fastsapp"],
    macos: MacConfig {
        bundle_ids: &["me.paolino.fastsapp"],
        executable_names: &[],
        legacy_bundle_names: &["FastsApp.app"],
    },
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    ..UpdateConfig::new("crmne/zapfast", "ZapFast", "zapfast", env!("CARGO_PKG_VERSION"))
};

fn main() {
    // First: run the helper if asked, and strip the update flags.
    let launch = fastframe_update::intercept(&UPDATES);
    let cli = Cli::parse_from(&launch.arguments);
    // Show `launch.error` as a toast; after the first frame, on a thread:
    // `launch.receipt.map(|receipt| receipt.acknowledge())`.
}

// On a worker thread, when the app decides to check:
let updater = Updater::new(UPDATES, ReqwestTransport::new(app_client_builder())?);
if let Some(release) = updater.check()? {
    match updater.installation() {
        Ok(_) => {
            let prepared = updater.download(&release, |received, total| { /* progress */ })?;
            // When the user clicks "Restart to update":
            updater.handoff(prepared, relaunch_arguments)?;
            // quit now
        }
        Err(reason) => { /* show `reason` and the release page */ }
    }
}
```

`env!("CARGO_PKG_VERSION")` must be expanded in the app. Inside this crate it
would report fastframe's version, so the app passes its own.

### What the app keeps

- When to check (`CHECK_INTERVAL` is one day), on which thread, and the
  setting that turns checks or automatic downloads off.
- Its HTTP client, with its proxy settings: implement `Transport`, or enable
  the `reqwest` feature and pass the app's `reqwest::blocking::ClientBuilder`
  to `ReqwestTransport::new`. A transport must not follow redirects; the
  updater follows them itself, only to the release hosts.
- The banner, dialog and wording. `Unsupported` has a variant per reason, so
  an app can translate them; its `Display` text is English.
- The restart action and the arguments the app relaunches with.
- The marker files in `packaging/`, the public key file, and the release
  workflow that produces `checksums.txt` and `checksums.txt.sig` (through
  native-packages' `sign-release`).
- `--version` printing `<slug> <version>`: the helper of the previous release
  runs the downloaded executable with `--version` before installing it.
- For an archive with more than one program, `portable_executable`: the
  app's file name inside it (TonePush's `tonepush-gui` beside its `tonepush`
  command-line tool). That file is unpacked, probed and installed, and only a
  running executable of that name updates itself. The probe still expects
  `<slug> <version>`. `None` means the slug.

## How an update is installed

1. `check` reads `https://api.github.com/repos/<repository>/releases/latest`
   and compares its tag with `current_version`. Pre-releases are never
   offered; a release candidate hears about its final release.
2. `installation` refuses copies a package manager owns: Flatpak, Snap,
   apt, dnf, pacman (including the AUR), Nix, Homebrew (formula paths and
   casks, which link to the bundle from `Caskroom`) and `cargo install`, and
   any Linux or Windows copy without a marker file next to it.
3. `download` reads the release by tag and requires the one asset named for
   this platform and installation, plus `checksums.txt` and, with a publisher
   key, `checksums.txt.sig`. The signature is checked before anything is
   parsed or written. The package is streamed into a new staging folder and
   hashed; a portable executable is unpacked with `tar` and must answer
   `--version` correctly; a macOS disk image is mounted and its bundle must
   have the same identifier, the release's version, the running app's
   signing team and Gatekeeper's approval.
4. `handoff` copies the running app into the staging folder (the whole
   bundle on macOS), writes `handoff.json` and starts the helper with
   `--apply-update`. It returns once the helper writes `ready`.
5. The helper waits for the app to exit, keeps a backup called `previous`,
   replaces the executable (portable), runs the setup program silently
   (Windows installer) or swaps the bundle (macOS), and relaunches the app
   with `--update-receipt`. The new app acknowledges by writing `started`.
   A failure at any step, or no `started` within a minute, restores
   `previous` and restarts the old app with `--update-error`.

## The contract between versions

An installed version runs the helper (a copy of itself) and the version it
installs reads the receipt. These must not change without a transition:

| Item | Value |
| --- | --- |
| Helper command | exactly `<app> --apply-update <path to handoff.json>` |
| Relaunch | `<arguments> --update-receipt <path to handoff.json>` |
| Restart after rollback | `<arguments> --update-error "The update could not start. The previous version has been restored."` |
| Staging folder | `.<slug>-update-<16 lowercase hex digits>` beside the executable, or beside the bundle on macOS; 0700 |
| Files in it | `handoff.json`, `ready`, `started`, `result.txt`, `previous`, `helper`/`helper.exe`/`helper.app`, `helper.log`, `installer.log`, `mounted-<hex>`, `failed.app` |
| `handoff.json` | compact JSON: `{"prepared":{"installation":{"executable":…,"kind":"Portable"\|"WindowsInstaller"\|"MacBundle"},"directory":…,"payload":…,"sha256":…,"version":…},"parent":<pid>,"arguments":[…]}` |
| Markers | `<name>-portable.txt` containing `<name>-portable-v1`; `<name>-installer.txt` containing `<name>-installer-v1` |
| Assets | `<slug>-v<version>-<target>.tar.gz` (Linux), `.zip` (Windows portable), `-setup.exe` (Windows installer), `<slug>-v<version>-macos-universal.dmg`; targets `x86_64`/`aarch64` `-unknown-linux-gnu` and `-pc-windows-msvc` |
| Archive layout | `<slug>-v<version>-<target>/<slug>[.exe]`, or `<portable_executable>[.exe]` in place of the second `<slug>` |
| Checksums | `checksums.txt` in `sha256sum` format; `checksums.txt.sig` is a raw 64-byte Ed25519 signature over its exact bytes |
| Version probe | `<slug or legacy name> <version>` on standard output |

`tests/fixtures` holds `handoff.json` files written by the `Handoff` structs
of ZapFast 0.16.3 and Spotifast 0.10.1 (including Spotifast's rewritten
receipt for a renamed macOS executable and a Fastpotify-era staging folder),
the real `checksums.txt` and signature of ZapFast 0.16.3 with its public key,
and the marker files both apps ship. The tests check that each job reads
back and writes out byte for byte, that the new app acknowledges old
receipts, and that the published signature verifies.

## Safety rules

- A `Prepared` update exists only as the result of `download`, is consumed
  by `handoff`, and cannot be deserialized: nothing installs from paths an
  app read back from a file. `Prepared::sample` is for demos, and a handoff
  refuses it.
- Every download gets a new staging folder. A handoff refuses a folder that
  holds anything from an earlier attempt, so a stale `ready` or `started`
  cannot fake a helper or skip rollback, and nothing is retried on its own.
- The helper and the receipt accept a job only when it sits in a staging
  folder with the expected name, beside the installation it names, with the
  payload inside it. The payload's hash is compared with the hash in the
  same folder: that catches corruption, not tampering, which the folder's
  0700 permissions and its place beside the executable guard against.
- A failed download removes its staging folder, unless a disk image may
  still be mounted in it.
- With a publisher key, a release without a valid signature is refused.
  There is no unsigned fallback and no key taken from the release.

### Why the macOS helper runs from a copy of the bundle

A signed Mach-O copied out of its bundle is killed on launch (ZapFast #129).
ZapFast fixed that by copying the whole bundle into the staging folder with
`ditto`; Spotifast by running the helper from the installed bundle itself.
This crate copies the bundle. The helper then runs from code the update never
moves: replacing the installed bundle, and restoring it on rollback, rename
the bundle out from under a live-bundle helper. It also keeps working when
the running bundle's name is not the one a release installs. The cost is one
bundle-sized copy in the staging folder.

## Platforms

Linux, macOS and Windows. Detection rules for all three are pure functions
over paths, marker files and an injected view of the environment, so their
tests run everywhere. Programs (`tar`, `dpkg-query`, `rpm`, `pacman`,
PlistBuddy, `codesign`, `spctl`, `hdiutil`, `ditto`, the Windows setup
program) and processes are reached through one internal trait that the tests
replace; the tests never spawn a process or use the network.

## License

MIT

## Rotating the publisher key

Keep signing with the current key while installs learn the next one:

1. Generate the next key, back it up outside GitHub, and put its public key
   in `additional_publisher_keys`. Releases stay signed with `publisher_key`.
2. Once most installs run a version that trusts both, sign releases with the
   next key and make it the `publisher_key`.

An install that skips every release in between refuses the newly signed one
and needs a manual download once.
