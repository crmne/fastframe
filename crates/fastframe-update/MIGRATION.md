# Moving an app onto fastframe-update

Each app moves in its own repository, as its own change. Pin fastframe by
revision:

```toml
fastframe-update = { git = "https://github.com/crmne/fastframe", rev = "<commit>", features = ["reqwest"] }
```

Both apps already build reqwest 0.12 with `blocking`, so the `reqwest`
feature adds nothing to their trees; ring, sha2 0.11, url, serde and anyhow
are already there too.

The first release built on the crate is installed by the previous release's
own updater, which is unchanged. What that old helper needs from the new
binary stays the same: `--version` prints `<slug> <version>`, the new app
accepts `--update-receipt <job>` and `--update-error <message>` (now through
`intercept`), and it writes `started` next to the receipt. The crate's
`compat` tests cover the receipts of both apps' current releases.

## ZapFast (0.16.x)

### Delete

| File | Lines |
| --- | --- |
| `src/updates.rs` | 123 |
| `src/updates/install.rs` | 798 |
| `src/updates/macos.rs` | 440 |
| `src/updates/signing.rs` | 85 |
| `src/updates/transfer.rs` | 498 |
| **Total** | **1,944** |

The 19 updater tests in those files move into the crate. Keep
`assets/update-public-key.hex`, `packaging/zapfast-portable.txt`,
`packaging/windows/zapfast-installer.txt` and `packaging/UPDATE_SIGNING.md`
(point its verification paragraph at this crate).

### Add

A new `src/updates.rs` of about 35 lines:

```rust
pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{MacConfig, ReqwestTransport, UpdateConfig};

pub const CONFIG: UpdateConfig = UpdateConfig {
    // Cask and bundle names from before the rename. This also accepts
    // fastsapp-* marker files and `fastsapp <version>` answers, which no
    // release produces.
    legacy_names: &["fastsapp"],
    macos: MacConfig {
        bundle_ids: &["me.paolino.fastsapp"],
        executable_names: &[],
        legacy_bundle_names: &["FastsApp.app"],
    },
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    ..UpdateConfig::new("crmne/zapfast", "ZapFast", "zapfast", env!("CARGO_PKG_VERSION"))
};

/// An updater on the proxy-aware reqwest client.
pub fn updater() -> anyhow::Result<Updater> {
    let mut builder = reqwest::blocking::Client::builder();
    if let Some(proxy) = crate::proxy::reqwest_proxy() {
        builder = builder.proxy(proxy);
    }
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}

#[test]
fn update_config_is_valid() {
    CONFIG.validate().unwrap();
}
```

### Replace

| Where | Today | With |
| --- | --- | --- |
| `main.rs` start | `if arguments.len() == 3 && arguments[1] == "--apply-update" { run_helper(..) }` | `let launch = fastframe_update::intercept(&updates::CONFIG);` |
| `main.rs` `Cli` | hidden `update_receipt` and `update_error` arguments; `Cli::parse()` | remove both; `Cli::parse_from(&launch.arguments)` |
| `main.rs` | `cli.update_error` toast | `launch.error` |
| `main.rs` | `update_receipt: Option<PathBuf>`, `update_receipt.is_none()` before starting hidden | `Option<fastframe_update::Receipt>`, `launch.receipt.is_none()` |
| `main.rs` `ui` | `install::acknowledge(&receipt)` | `receipt.acknowledge()` |
| `worker.rs` `InspectUpdate` | `install::detect()` | `updates::updater()?.installation()` (map the error with `to_string()`) |
| `worker.rs` `DownloadUpdate` | `updates::download(&release, &source, progress)` | `updater.with_source(source).download(&release, progress)` |
| `worker.rs` `InstallUpdate` | `install::handoff(&prepared, arguments)` | `updater.handoff(*prepared, arguments)` |
| `worker.rs` `CheckForUpdates` | `updates::newer_release()` over `proxy::agent()` | `updater.check()` |
| `app.rs`, `demo.rs` tests | `Prepared { installation, directory, payload, sha256, version }` | `Prepared::sample(installation, "99.0.0")` |
| `Source::GitHub` | enum variant | `Source::github()`; `source.is_github()` |

`Command::InstallUpdate` already moves the `Prepared` out of
`DownloadState::Ready` with `mem::replace`, which suits the non-`Clone`
`Prepared`.

Net: about 1,944 lines deleted and 60 added or changed, so roughly 1,880
fewer lines.

### What changes for users

- The helper's standard error goes to `helper.log` in the staging folder (as
  Spotifast does), and `handoff` names that file when the helper dies.
- A Nix install says "Update this installation with Nix." instead of "with
  Nix or Homebrew".
- A handoff into a staging folder that already holds a marker is refused
  (none can today, since each download gets a new folder).
- `result.txt` is written before the rolled-back app restarts, not after.

## Spotifast (0.10.x)

### Delete

| File | Lines |
| --- | --- |
| `src/updates.rs` | 122 |
| `src/updates/install.rs` | 791 |
| `src/updates/macos.rs` | 503 |
| `src/updates/transfer.rs` | 587 |
| **Total** | **2,003** |

The 21 updater tests move into the crate. Keep the marker files in
`packaging/` (`spotifast-` and `fastpotify-portable.txt`,
`spotifast-` and `fastpotify-installer.txt`) and the CI step that runs
`--apply-update` on a missing job inside a real bundle: `intercept` prints
the error to standard error and exits with 1, as today.

`examples/updater-inspect.rs` (11 lines) becomes a call to
`Updater::installation_at(&path)`.

### Add

A new `src/updates.rs` of about 40 lines:

```rust
pub use fastframe_update::{
    CHECK_INTERVAL, DownloadState, Installation, Kind, Prepared, Release, Source, Unsupported,
    Updater,
};
use fastframe_update::{MacConfig, ReqwestTransport, UpdateConfig};

pub const CONFIG: UpdateConfig = UpdateConfig {
    legacy_names: &["fastpotify"],
    legacy_windows_installs: &["Programs/Fastpotify/fastpotify.exe"],
    macos: MacConfig {
        bundle_ids: &["rocks.spotifast.Spotifast", "me.paolino.fastpotify"],
        // 0.9.1 kept "fastpotify" for older clients' validation; later
        // releases may rename it to "Spotifast" (#538).
        executable_names: &["fastpotify", "Spotifast"],
        legacy_bundle_names: &["Fastpotify.app"],
    },
    // Checksums only until releases are signed; see below.
    publisher_key: None,
    ..UpdateConfig::new("crmne/spotifast", "Spotifast", "spotifast", env!("CARGO_PKG_VERSION"))
};

pub fn updater(proxy: &crate::settings::ProxyConfig) -> anyhow::Result<Updater> {
    let builder = crate::http::blocking_builder(proxy).map_err(anyhow::Error::msg)?;
    Ok(Updater::new(CONFIG, ReqwestTransport::new(builder)?))
}
```

### Replace

| Where | Today | With |
| --- | --- | --- |
| `entrypoint.rs` start | `--apply-update` block with `eprintln!` and `exit` | `let launch = fastframe_update::intercept(&updates::CONFIG);` (keep it before the MilkDrop child check) |
| `entrypoint.rs` `Cli` | hidden `update_receipt`/`update_error`; `Cli::command().name(name).get_matches()` | remove both; `.get_matches_from(&launch.arguments)`. The `fastpotify`/`spotifast` name choice from `argv[0]` stays: old updaters check `fastpotify <version>`. |
| `entrypoint.rs` | `cli.update_receipt.is_some()` for migration and `AppDirs::for_launch` | `launch.receipt.is_some()` |
| `entrypoint.rs` demo feed | `updates::Source::local(feed)` | `fastframe_update::Source::local(feed)` |
| `backend.rs` `check_for_updates` | async `newer_release_from(&http, &source)` | `tokio::task::spawn_blocking(move \|\| updater.with_source(source).check())` |
| `backend.rs` `InspectUpdate` | `install::detect()` | `updater.installation()` |
| `backend.rs` `DownloadUpdate` | `updates::download(&release, &source, &proxy, progress)` | `updates::updater(&proxy)?.with_source(source).download(&release, progress)` |
| `backend.rs` `InstallUpdate` | `install::handoff(&prepared, arguments)` | `updater.handoff(*prepared, arguments)` |
| `app.rs` `InstallUpdate` | `prepared.clone()` from `&self.update_download` | move it out with `std::mem::replace(&mut self.update_download, DownloadState::Installing)`; `Prepared` is not `Clone` |
| `app.rs` | `matches!(self.update_source, Source::GitHub)` | `self.update_source.is_github()` |
| `app.rs` receipt | `install::acknowledge(&receipt)` | `receipt.acknowledge()` |
| `demo.rs`, `app.rs` tests | `Prepared { .. }` literals | `Prepared::sample(installation, version)` |

Net: about 2,003 lines deleted and 70 added or changed, so roughly 1,930
fewer lines.

### What changes for users

- **The macOS helper runs from a copy of the whole bundle** (ZapFast's fix),
  not from the installed bundle. The installed bundle can then be moved and
  restored without touching the helper's own code.
- `portable_entry`'s special case for the 0.8.0 and 0.9.0 archive layout is
  gone. Those versions are older than 0.10, so no app built on the crate can
  be offered them.
- The pre-marker Windows location now also checks the setup program's
  current default, `Programs/Spotifast/spotifast.exe`, as well as
  `Programs/Fastpotify/fastpotify.exe`.
- A Nix install says "Update this installation with Nix."

### Turning on publisher signatures

Spotifast releases are not signed yet, so `publisher_key` stays `None` and
the update is verified against `checksums.txt` alone. To sign:

1. Add native-packages' `sign-release` step and a `release-signing`
   environment with the private key to the release workflow, as ZapFast has,
   and publish a release that carries `checksums.txt.sig`.
2. Only then ship a version with `publisher_key: Some(include_str!(..))`.
   From that version on, a release without a valid signature is refused, so
   every later release must be signed.

Older installs keep updating on checksums until they reach a version that
embeds the key.

## Deleted lines, both apps

About 3,950 lines of updater code and tests leave the two apps, replaced by
about 75 lines of configuration and 130 of changed call sites. The crate is
about 6,000 lines as formatted: about 2,900 of code and documentation and
3,100 of tests and test fakes.
