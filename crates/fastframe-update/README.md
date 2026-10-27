# fastframe-update

Self-update from GitHub releases, or from the app's own HTTPS feed, for
desktop apps: check for a newer release, decide whether this copy may replace
itself, download and verify the update, and hand it to a helper that installs
it, relaunches the app and rolls back if the new version does not start.

Extracted from ZapFast (0.16) and Spotifast (0.10), whose updaters had
drifted apart. It keeps ZapFast's publisher signatures and whole-bundle macOS
helper, and Spotifast's legacy names, renamed macOS executables and helper
log. Installations under a legacy name move onto the new one as they
update (see [Renamed apps](#renamed-apps)).

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
   offered; a release candidate hears about its final release. On the
   pre-release channel (below) it reads the release list instead.
2. `installation` refuses copies a package manager owns: Flatpak, Snap,
   apt, dnf, pacman (including the AUR), Nix, Homebrew (formula paths and
   casks, which link to the bundle from `Caskroom`) and `cargo install`, and
   any Linux or Windows copy without a marker file next to it. With
   `appimage: true`, an AppImage the user owns updates itself (below).
3. `download` reads the release by tag and requires the one asset named for
   this platform and installation, plus `checksums.txt` and, with a publisher
   key, `checksums.txt.sig`. The checksum file and signature are fetched
   concurrently. The signature is checked before anything is parsed or
   written. Package writes are batched in a bounded 256 KiB buffer, flushed
   and synced before validation. Unchanged packages retain the digest verified
   during the stream; extracted portable executables are hashed separately.
   Handoff and installation still rehash the staged payload to reject changes.
   The package is streamed into a new staging
   folder and hashed; a portable executable is unpacked with `tar` and must answer
   `--version` correctly; a macOS disk image is mounted and its bundle must
   have the same identifier, the release's version, the running app's
   signing team and Gatekeeper's approval.
   Disk images mount read-only without repeating their internal image checksum:
   the updater has already verified the signed SHA-256 before mounting, and
   the helper rechecks the staged SHA-256 before mounting during installation.
4. `handoff` copies the running app into the staging folder (the whole
   bundle on macOS, and on Windows the DLLs beside the executable too),
   writes `handoff.json` and starts the helper with `--apply-update`, working
   in the staging folder. It returns once the helper writes `ready`.
   macOS bundle copies remove only FinderInfo and ResourceFork attributes,
   which strict code-signature verification rejects. Quarantine, provenance,
   other attributes, file bytes and signatures are preserved; no re-signing
   takes place.
   See [Apple QA1940](https://developer.apple.com/library/archive/qa/qa1940/_index.html)
   for the signature failure these attributes cause.
5. The helper waits for the app to exit, keeps a backup called `previous`,
   replaces the executable (portable), runs the installer silently
   (Windows installer, below) or swaps the bundle (macOS), and relaunches the app
   with `--update-receipt`, working in the app's folder. The new app
   acknowledges by writing `started`.
   A failure at any step, or no `started` within a minute, restores
   `previous` and restarts the old app with `--update-error`. A portable
   update touches only the executable and its staging folder, so the
   `<slug>-data` folder a portable copy keeps beside it on Windows
   (`fastframe-dirs`) stays as it was, whether the update sticks, rolls
   back or moves the copy off a legacy name.
6. Staging folders do not pile up. When the new app acknowledges, and
   before each download, the staging folders of earlier updates go: those
   whose helper wrote `result.txt`, and those no helper ever took. A folder
   whose helper is still working stays, unless its `ready` is a day old.
   The latest update's folder stays until the next update or download, so
   an uninstaller should remove `.<slug>-update-*` folders itself.

## Windows Installer packages

A Windows installation updates through `<slug>-v<version>-<target>.msi`
when the release has one, and through `-setup.exe` (Inno Setup) when it
does not. The package is verified against the signed checksums like any
other asset. The helper runs

```text
%SystemRoot%\System32\msiexec.exe /i "<package>" /qn /norestart /l*v "<staging folder>\installer.log" INSTALLFOLDER="<folder the app runs from>"
```

and counts exit codes 0 and 3010 (a restart is due) as installed. Anything
else rolls back as a failed setup program does. Everything around it is
unchanged: the helper waits for the app to exit, keeps `previous`, and
relaunches the app for its receipt.

What the app's package must do:

- Install per user, so no elevation is asked for, with its folder's
  directory ID `INSTALLFOLDER`: the updater installs into the folder the
  app runs from, wherever that is.
- Install the `<slug>-installer.txt` marker beside the executable, as the
  setup program does. Detection treats both the same.
- Take over a copy that its Inno Setup program installed, so the first
  update to the package (and a package run by hand over an Inno install)
  ends with one installation: Inno's uninstall entry gone, the package's
  present, settings and data kept. The updater does not run Inno's
  uninstaller.

An app moves over in two releases. The first ships both the setup program
and the package: older versions only know `-setup.exe` and update to it,
and that release's updater prefers the package. Releases after it can ship
the package alone.

## AppImages

An app whose release lists `<slug>-<version>-<arch>.AppImage` (`x86_64` or
`aarch64`, the name native-packages gives it) in the signed
`checksums.txt` turns AppImage updates on:

```rust
pub const UPDATES: UpdateConfig = UpdateConfig {
    appimage: true,
    ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", env!("CARGO_PKG_VERSION"))
};
```

- A copy runs from an AppImage when `$APPIMAGE` is an absolute path and the
  running executable is inside `$APPDIR`, both of which the AppImage
  runtime sets. A program an AppImage started inherits the variables but
  runs from elsewhere, so it never takes that AppImage for its own.
- The AppImage is refused when dpkg, rpm or pacman owns it, when it is under
  `/usr`, `/bin` or `/sbin`, when it is not a regular file, and with
  `Unsupported::NotWritable` when the user does not own it, cannot write
  it, or cannot write to its folder (a folder of the user's own, or one
  writable by everyone). Flatpak and Snap are refused first, as always.
- The download is the AppImage itself, verified against the signed
  checksums like every other download. It takes the running AppImage's
  permissions and must answer `--version` with `<slug> <version>`.
- The helper is a copy of the AppImage, since the running executable lives
  in the AppImage's mount, which goes when the app quits. It backs the
  AppImage up as `previous` and renames the download over it: the staging
  folder is beside the AppImage, on the same file system, so anything that
  starts it meanwhile gets the old file or the new one. A rollback renames
  `previous` back, since Linux refuses to write into an AppImage that is
  still running.
- The relaunched app's receipt matches when `$APPIMAGE` names the installed
  file.
- An AppImage unpacked with `--appimage-extract-and-run` (no FUSE) runs from
  an `appimage_extracted_*` folder. Its version probe, helper and relaunch
  get `APPIMAGE_EXTRACT_AND_RUN=1`, so they unpack themselves the same way.
- The download is the whole file. External updaters (AppImageUpdate and
  those built on it) can do delta updates from the `.zsync` files a release
  publishes beside its AppImages, through the update information embedded
  in each; the crate does not read either.
- Without `appimage: true`, an AppImage is refused with `NotPortable`, as
  before.

## Pre-releases

An app whose releases are all pre-releases (Solco's alphas, for which
`releases/latest` answers 404) opts in:

```rust
use fastframe_update::{Prereleases, UpdateConfig};

pub const UPDATES: UpdateConfig = UpdateConfig {
    prereleases: Prereleases::WhenRunningPrerelease,
    ..UpdateConfig::new("crmne/solco-releases", "Solco", "solco", env!("CARGO_PKG_VERSION"))
};
```

- The default, `Prereleases::Never`, is the behaviour described above.
- With `WhenRunningPrerelease`, a build whose own version is a pre-release
  (`0.3.0-alpha.4`) reads `/repos/<repository>/releases` (100 per page, at
  most three pages), skips drafts and any tag that is not `v` followed by a
  valid version, and offers the highest version above its own by semver
  precedence: a newer pre-release (`alpha.10` after `alpha.9`) or a stable
  release (`0.3.0` after `0.3.0-rc.1`), whatever GitHub's pre-release flag
  says. A stable build with the same setting still reads only
  `releases/latest` and is never offered a pre-release.
- Versions on the channel must be `major.minor.patch[-identifiers]`, with
  numbers without leading zeros and dot-separated identifiers of ASCII
  letters, digits and hyphens, at most 64 bytes. No `v`, no `+build`
  metadata, no empty identifiers. That keeps every version safe in asset
  names, staging files and URLs. Anything else is skipped when listing and
  refused by `download` before any request.
- Everything downstream takes the full version: assets are
  `<slug>-v0.3.0-alpha.10-<target>…`, `--version` must print
  `<slug> 0.3.0-alpha.10`, and the receipt carries that version. The helper
  and the receipt accept a pre-release version only when their own build is
  on the channel. A macOS bundle may carry just `0.3.0` as its
  `CFBundleShortVersionString`, since macOS expects three numbers there; the
  `--version` probe still checks the exact version.
- Nothing about the file formats changes: `handoff.json`, the markers and the
  staging names are the same as for stable releases.
- A local feed (`Source::local`) or the app's own feed (`Source::feed`)
  serves the list as `<base>/releases.json` (one page) and the chosen
  stable release's metadata as `<base>/latest.json`. On the pre-release
  channel, the chosen version's metadata and assets come from its entry in
  `releases.json`, so one feed can serve both channels.

## An app's own feed

An app whose repository is private cannot use GitHub's release API without
a token, so it can serve its releases itself and point the updater there:

```rust
let updater = Updater::new(UPDATES, transport).with_source(Source::feed("https://example.com/app")?);
```

`<base>/latest.json` is shaped like GitHub's release, with only the fields
the updater reads:

```json
{
  "tag_name": "v1.2.3",
  "html_url": "https://example.com/app",
  "draft": false,
  "prerelease": false,
  "assets": [
    { "name": "app-v1.2.3-macos-universal.dmg", "browser_download_url": "https://example.com/app/v1.2.3/app-v1.2.3-macos-universal.dmg", "size": 15000000 },
    { "name": "checksums.txt", "browser_download_url": "https://example.com/app/v1.2.3/checksums.txt", "size": 106 },
    { "name": "checksums.txt.sig", "browser_download_url": "https://example.com/app/v1.2.3/checksums.txt.sig", "size": 64 }
  ]
}
```

The feed must be HTTPS, and every download must come from its origin:
redirects elsewhere are refused, so the server streams the files. The names,
checksums and publisher signature are the same as on GitHub.

`UpdateConfig::repository` is not read with a feed, but it must still be
`owner/name`.

A feed needs a publisher key: `check` and `download` refuse one when
`UpdateConfig::publisher_key` is `None`. Checksums served by the same server
as the files only catch corruption; the signature is what stops a server
that was broken into from shipping its own build.

## Apple silicon only

An app that ships no Intel build publishes `<slug>-v<version>-macos-arm64.dmg`
instead of the universal image and sets `mac_target: MacTarget::Arm64Only`.
The default, `MacTarget::Universal`, keeps `macos-universal`. With
`Arm64Only`, `installation` refuses a copy running as x86_64 with
`Unsupported::Platform` before anything is downloaded. That is the
architecture of the running executable (`std::env::consts::ARCH`): an arm64
executable only runs on Apple silicon, and an x86_64 one (an Intel Mac, or
an older universal build under Rosetta) has no image to update to. Linux and
Windows asset names do not change.

## Renamed apps

An app that changed its name lists the old ones in `legacy_names` (and, on
macOS, `macos.legacy_bundle_names` and `macos.executable_names`). Their
marker files, staging folders and `--version` answers keep working, and
each update moves an installation that still runs under an old name onto
the new one:

- **Windows installer.** The setup program installs `<slug>.exe`. When the
  helper was started from a legacy executable, it relaunches `<slug>.exe`
  instead, and deletes the legacy executable once that start is
  acknowledged. An app that starts as `<slug>.exe` with no update in
  progress deletes legacy executables beside it too (from `intercept`),
  since a helper from before this change relaunches the executable it was
  started from: the app's setup program has to keep installing a copy under
  the legacy name, which the next start removes.
- **Portable copy (Linux, Windows).** The update is written as `<slug>`
  beside the old file, unless something already has that name or the app
  sets `portable_executable`. On Unix the old name becomes a symbolic link
  to it, so scripts that name it keep working; on Windows it is deleted.
- **macOS.** A bundle with a legacy bundle name is installed as
  `<app_name>.app` in the same folder.

A failed start rolls back under the old name, and removes the new one.
Once a rename sticks, the launchers that named the old path exactly are
repointed at the new one: on Linux the `.desktop` files in
`$XDG_DATA_HOME/applications`, `$XDG_CONFIG_HOME/autostart` and
`~/Desktop` whose `Exec` or `TryExec` runs it, and the symbolic links in
`~/.local/bin` and `~/bin`; on Windows the shortcuts in the Start menu
(with the Startup folder), on the desktop and pinned to the taskbar,
through Windows PowerShell. Scripts and other files are left alone. A
launcher that cannot be rewritten keeps its old target, and never fails an
update.

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
| `handoff.json` | compact JSON: `{"prepared":{"installation":{"executable":…,"kind":"Portable"\|"WindowsInstaller"\|"MacBundle"\|"AppImage"},"directory":…,"payload":…,"sha256":…,"version":…},"parent":<pid>,"arguments":[…]}`. `AppImage` is newer than the others: only a helper that knows it writes it, and only the newer app it installs reads it back. |
| Markers | `<name>-portable.txt` containing `<name>-portable-v1`; `<name>-installer.txt` containing `<name>-installer-v1` |
| Assets | `<slug>-v<version>-<target>.tar.gz` (Linux), `.zip` (Windows portable), `.msi` or else `-setup.exe` (Windows installer), `<slug>-v<version>-macos-universal.dmg` (or `-macos-arm64.dmg` with `MacTarget::Arm64Only`), `<slug>-<version>-<arch>.AppImage` (with `appimage: true`; `x86_64`, `aarch64`); targets `x86_64`/`aarch64` `-unknown-linux-gnu` and `-pc-windows-msvc` |
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

### Why the Windows helper takes the app's DLLs

Windows looks for a program's DLLs in its own folder, then the system's,
then its working directory. The helper's copy sits in the staging folder, so
without its own copies of the DLLs beside the app's executable (Solco's
`webgpu_dawn.dll`, say) it either fails to start or, working in the app's
folder as a Start menu shortcut leaves it, loads the app's DLLs and holds
them. The installer then finds those files in use and gives up. With the
copies, and working in the staging folder, the helper holds nothing the
update replaces.

## Platforms

Linux, macOS and Windows. Detection rules for all three are pure functions
over paths, marker files and an injected view of the environment, so their
tests run everywhere. Programs (`tar`, `dpkg-query`, `rpm`, `pacman`,
PlistBuddy, `codesign`, `spctl`, `hdiutil`, `ditto`, `msiexec`, the Windows setup
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
