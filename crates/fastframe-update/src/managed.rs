//! How to update a copy that a package manager owns.
//!
//! The updater never replaces such a copy ([`Unsupported`]), but the app can
//! still tell people a newer release is out and how to get it there: the
//! package manager, the package's name, and the command that updates it,
//! for the app to show in its own words (with a Copy button, say). Nothing
//! here runs that command.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::detect::{PackageManager, Platform, Unsupported};
use crate::host::Host;

/// The tool that updates this copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Manager {
    /// An Arch package from a sync repository.
    Pacman,
    /// An Arch package that no sync repository has: the AUR, or a package
    /// built by hand. Its command names the AUR helper found (`yay`, else
    /// `paru`, else `yay`).
    Aur,
    /// A Debian or Ubuntu package.
    Apt,
    /// An RPM package on Fedora and its relatives.
    Dnf,
    /// An RPM package on openSUSE.
    Zypper,
    /// A Homebrew formula.
    Homebrew,
    /// A Homebrew cask.
    HomebrewCask,
    /// A Flatpak.
    Flatpak,
    /// A Snap.
    Snap,
    /// Nix: a `nix profile` element, or a system or Home Manager
    /// configuration (no command then).
    Nix,
    /// `cargo install`.
    Cargo,
}

/// How to update a copy a package manager owns. Every field is something
/// the computer said; the app words the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ManagedUpdate {
    /// The tool that updates it.
    pub manager: Manager,
    /// The package, formula, cask, Flatpak or Snap name, `nix profile`
    /// element or crate, when the computer tells.
    pub package: Option<String>,
    /// The command that updates it, ready to paste into a terminal. Only
    /// plain names reach it (letters, digits and `-_.+@`, or a URL for a
    /// crate installed from git). `None` when there is no one command.
    pub command: Option<String>,
}

/// How to update `executable`, given why it can't update itself. `None`
/// when no package manager owns it.
pub(crate) fn describe(
    reason: &Unsupported,
    executable: &Path,
    host: &dyn Host,
    platform: Platform,
) -> Option<ManagedUpdate> {
    let path = executable.as_os_str();
    let query = |program: &str, args: &[&OsStr]| host.query(program, args);
    let (manager, package) = match reason {
        Unsupported::SystemPackage(PackageManager::Pacman) => {
            let name = query("pacman", &["-Qqo".as_ref(), path]).filter(|n| plain(n));
            // -Qm lists the packages no sync repository has.
            let foreign = name
                .as_deref()
                .is_some_and(|name| query("pacman", &["-Qqm".as_ref(), name.as_ref()]).is_some());
            let manager = if foreign {
                Manager::Aur
            } else {
                Manager::Pacman
            };
            (manager, name)
        }
        Unsupported::SystemPackage(PackageManager::Apt) => (
            Manager::Apt,
            query("dpkg-query", &["-S".as_ref(), path]).and_then(|out| dpkg_package(&out)),
        ),
        Unsupported::SystemPackage(PackageManager::Dnf) => {
            // dnf or zypper by the distribution; without /etc/os-release
            // there's no telling, so no command.
            let release = host.read_text(Path::new("/etc/os-release"));
            let manager = if release.as_deref().is_some_and(opensuse) {
                Manager::Zypper
            } else {
                Manager::Dnf
            };
            let name = query(
                "rpm",
                &["-qf".as_ref(), "--qf".as_ref(), "%{NAME}".as_ref(), path],
            )
            .filter(|n| plain(n));
            if release.is_none() {
                return Some(ManagedUpdate {
                    manager,
                    package: name,
                    command: None,
                });
            }
            (manager, name)
        }
        Unsupported::Homebrew => {
            let (cask, name) = homebrew(executable)?;
            (
                if cask {
                    Manager::HomebrewCask
                } else {
                    Manager::Homebrew
                },
                Some(name),
            )
        }
        Unsupported::Flatpak => (
            Manager::Flatpak,
            var(host, "FLATPAK_ID").filter(|id| plain(id)),
        ),
        Unsupported::Snap => (
            Manager::Snap,
            var(host, "SNAP_INSTANCE_NAME")
                .or_else(|| var(host, "SNAP_NAME"))
                .filter(|name| plain(name)),
        ),
        Unsupported::Nix => (Manager::Nix, nix_element(executable, host)),
        Unsupported::Cargo => {
            let home = var(host, "CARGO_HOME")
                .map(PathBuf::from)
                .or_else(|| home(host, platform).map(|home| home.join(".cargo")))?;
            let crate_ = host
                .read_text(&home.join(".crates2.json"))
                .and_then(|text| cargo_crate(&text, executable));
            return Some(ManagedUpdate {
                manager: Manager::Cargo,
                command: crate_.as_ref().and_then(CargoCrate::command),
                package: crate_.map(|c| c.name),
            });
        }
        _ => return None,
    };
    let helper = || {
        if host.query("yay", &["--version".as_ref()]).is_some() {
            "yay"
        } else if host.query("paru", &["--version".as_ref()]).is_some() {
            "paru"
        } else {
            "yay"
        }
    };
    let command = package.as_deref().map(|name| match manager {
        Manager::Pacman => format!("sudo pacman -Syu {name}"),
        Manager::Aur => format!("{} -Syu {name}", helper()),
        Manager::Apt => format!("sudo apt update && sudo apt install --only-upgrade {name}"),
        Manager::Dnf => format!("sudo dnf upgrade --refresh {name}"),
        Manager::Zypper => format!("sudo zypper refresh && sudo zypper update {name}"),
        Manager::Homebrew => format!("brew upgrade {name}"),
        Manager::HomebrewCask => format!("brew upgrade --cask {name}"),
        Manager::Flatpak => format!("flatpak update {name}"),
        Manager::Snap => format!("sudo snap refresh {name}"),
        Manager::Nix => format!("nix profile upgrade {name}"),
        Manager::Cargo => format!("cargo install {name}"),
    });
    Some(ManagedUpdate {
        manager,
        package,
        command,
    })
}

fn var(host: &dyn Host, key: &str) -> Option<String> {
    host.var(key)
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
}

fn home(host: &dyn Host, platform: Platform) -> Option<PathBuf> {
    let key = if platform == Platform::Windows {
        "USERPROFILE"
    } else {
        "HOME"
    };
    var(host, key).map(PathBuf::from)
}

/// A name that is safe to paste into a shell as one word.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.+@".contains(&b))
}

/// The package in `dpkg-query -S`'s answer: `name: /path` or
/// `name:arch: /path`, the first line when several packages share a folder.
fn dpkg_package(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let (packages, _) = line.split_once(": ")?;
    let name = packages.split(", ").next()?;
    let name = name.split(':').next()?;
    plain(name).then(|| name.to_owned())
}

/// Whether `/etc/os-release` describes openSUSE (or SUSE).
fn opensuse(os_release: &str) -> bool {
    os_release.lines().any(|line| {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        matches!(key, "ID" | "ID_LIKE")
            && value
                .trim_matches('"')
                .split_whitespace()
                .any(|id| id.starts_with("opensuse") || id == "suse" || id == "sles")
    })
}

/// Whether a Homebrew path is a cask, and the formula or cask name: the
/// folder after `Cellar` or `Caskroom`.
fn homebrew(executable: &Path) -> Option<(bool, String)> {
    let mut components = executable.components();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            continue;
        };
        let cask = match name.to_str()?.to_ascii_lowercase().as_str() {
            "cellar" => false,
            "caskroom" => true,
            _ => continue,
        };
        let Some(Component::Normal(package)) = components.next() else {
            return None;
        };
        let package = package.to_str()?;
        return plain(package).then(|| (cask, package.to_owned()));
    }
    None
}

/// The `nix profile` element holding `executable`'s store path, from the
/// user's profile manifest: by name (manifest version 3), else by index.
fn nix_element(executable: &Path, host: &dyn Host) -> Option<String> {
    // Store paths are written with `/`, whatever the system's separator.
    let text = executable.to_string_lossy().replace('\\', "/");
    let store: String = text.split('/').take(4).collect::<Vec<_>>().join("/");
    if !store.starts_with("/nix/store/") || store.len() <= "/nix/store/".len() {
        return None;
    }
    let home = var(host, "HOME")?;
    let state = var(host, "XDG_STATE_HOME").unwrap_or_else(|| format!("{home}/.local/state"));
    // The profile that holds it: ~/.nix-profile, or the XDG one when
    // ~/.nix-profile is stale or absent.
    [
        format!("{home}/.nix-profile/manifest.json"),
        format!("{state}/nix/profile/manifest.json"),
    ]
    .iter()
    .filter_map(|path| host.read_text(Path::new(path)))
    .find_map(|manifest| element_in(&manifest, &store))
}

/// The element of a profile manifest holding `store`.
fn element_in(manifest: &str, store: &str) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_str(manifest).ok()?;
    let holds = |element: &serde_json::Value| {
        element["storePaths"]
            .as_array()
            .is_some_and(|paths| paths.iter().any(|p| p.as_str() == Some(store)))
    };
    match &manifest["elements"] {
        serde_json::Value::Object(elements) => elements
            .iter()
            .find(|(_, element)| holds(element))
            .map(|(name, _)| name.clone())
            .filter(|name| plain(name)),
        serde_json::Value::Array(elements) => elements
            .iter()
            .position(holds)
            .map(|index| index.to_string()),
        _ => None,
    }
}

/// A crate `cargo install` put there, and where it came from.
#[derive(Debug, PartialEq, Eq)]
struct CargoCrate {
    name: String,
    source: CrateSource,
}

#[derive(Debug, PartialEq, Eq)]
enum CrateSource {
    CratesIo,
    /// An HTTPS git repository, for a crate installed with `--git`.
    Git(String),
    /// Anything else (another registry, SSH git, a path): no one command.
    Other,
}

impl CargoCrate {
    fn command(&self) -> Option<String> {
        match &self.source {
            CrateSource::CratesIo => Some(format!("cargo install {}", self.name)),
            CrateSource::Git(url) => Some(format!("cargo install --git {url} {}", self.name)),
            CrateSource::Other => None,
        }
    }
}

/// The crate in `~/.cargo/.crates2.json` whose binaries include
/// `executable`'s file name. Its keys read `name version (source)`.
fn cargo_crate(crates2: &str, executable: &Path) -> Option<CargoCrate> {
    let binary = executable.file_name()?.to_str()?;
    let binary_stem = binary.strip_suffix(".exe").unwrap_or(binary);
    let json: serde_json::Value = serde_json::from_str(crates2).ok()?;
    let (key, _) = json["installs"].as_object()?.iter().find(|(_, install)| {
        install["bins"].as_array().is_some_and(|bins| {
            bins.iter().any(|bin| {
                bin.as_str().is_some_and(|bin| {
                    bin == binary
                        || bin.strip_suffix(".exe") == Some(binary_stem)
                        || bin == binary_stem
                })
            })
        })
    })?;
    let mut parts = key.splitn(3, ' ');
    let name = parts.next().filter(|name| plain(name))?.to_owned();
    let source = parts
        .nth(1)
        .unwrap_or_default()
        .trim_matches(|c| c == '(' || c == ')');
    let source = if matches!(
        source,
        "registry+https://github.com/rust-lang/crates.io-index" | "sparse+https://index.crates.io/"
    ) {
        CrateSource::CratesIo
    } else {
        source
            .strip_prefix("git+")
            .and_then(|url| url::Url::parse(url).ok())
            .filter(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            })
            .map(|mut url| {
                url.set_query(None);
                url.set_fragment(None);
                url.to_string()
            })
            .filter(|url| {
                url.bytes()
                    .all(|b| b.is_ascii_graphic() && !b"'\"`$\\;&|<>(){}".contains(&b))
            })
            .map_or(CrateSource::Other, CrateSource::Git)
    };
    Some(CargoCrate { name, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;

    fn on(reason: Unsupported, executable: &str, host: &FakeHost) -> Option<ManagedUpdate> {
        describe(&reason, Path::new(executable), host, Platform::Linux)
    }

    fn command(update: Option<ManagedUpdate>) -> Option<String> {
        update.and_then(|update| update.command)
    }

    #[test]
    fn arch_packages_name_pacman_or_the_aur_helper() {
        let pacman = Unsupported::SystemPackage(PackageManager::Pacman);
        let repo = FakeHost::default().answering("pacman -Qqo", "zapfast");
        let update = on(pacman.clone(), "/usr/bin/zapfast", &repo).unwrap();
        assert_eq!(update.manager, Manager::Pacman);
        assert_eq!(update.package.as_deref(), Some("zapfast"));
        assert_eq!(update.command.as_deref(), Some("sudo pacman -Syu zapfast"));

        let aur = FakeHost::default()
            .answering("pacman -Qqo", "zapfast-bin")
            .answering("pacman -Qqm", "zapfast-bin");
        let update = on(pacman.clone(), "/usr/bin/zapfast", &aur).unwrap();
        assert_eq!(update.manager, Manager::Aur);
        assert_eq!(update.command.as_deref(), Some("yay -Syu zapfast-bin"));
        let paru = FakeHost::default()
            .answering("pacman -Qqo", "zapfast-bin")
            .answering("pacman -Qqm", "zapfast-bin")
            .answering("paru --version", "paru v2.1.0");
        assert_eq!(
            command(on(pacman.clone(), "/usr/bin/zapfast", &paru)).as_deref(),
            Some("paru -Syu zapfast-bin")
        );
        // A name that isn't plain never reaches a command.
        let odd = FakeHost::default().answering("pacman -Qqo", "x; rm -rf ~");
        let update = on(pacman, "/usr/bin/zapfast", &odd).unwrap();
        assert_eq!(update.package, None);
        assert_eq!(update.command, None);
    }

    #[test]
    fn debian_packages_upgrade_only_themselves() {
        let apt = Unsupported::SystemPackage(PackageManager::Apt);
        for answer in [
            "zapfast: /usr/bin/zapfast",
            "zapfast:amd64: /usr/bin/zapfast",
            "zapfast, other: /usr/bin\nmore: /usr/bin",
        ] {
            let host = FakeHost::default().answering("dpkg-query -S", answer);
            assert_eq!(
                command(on(apt.clone(), "/usr/bin/zapfast", &host)).as_deref(),
                Some("sudo apt update && sudo apt install --only-upgrade zapfast"),
                "{answer}"
            );
        }
    }

    #[test]
    fn rpm_packages_use_zypper_on_opensuse_and_dnf_elsewhere() {
        let rpm = Unsupported::SystemPackage(PackageManager::Dnf);
        let fedora = FakeHost::default()
            .answering("rpm -qf", "spotifast")
            .with_file("/etc/os-release", "NAME=\"Fedora Linux\"\nID=fedora\n");
        let update = on(rpm.clone(), "/usr/bin/spotifast", &fedora).unwrap();
        assert_eq!(update.manager, Manager::Dnf);
        assert_eq!(
            update.command.as_deref(),
            Some("sudo dnf upgrade --refresh spotifast")
        );
        // Without /etc/os-release: dnf or zypper can't be told, so no command.
        let unknown = FakeHost::default().answering("rpm -qf", "spotifast");
        let update = on(rpm.clone(), "/usr/bin/spotifast", &unknown).unwrap();
        assert_eq!(update.package.as_deref(), Some("spotifast"));
        assert_eq!(update.command, None);
        for release in [
            "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
            "ID=\"opensuse-leap\"\n",
        ] {
            let suse = FakeHost::default()
                .answering("rpm -qf", "spotifast")
                .with_file("/etc/os-release", release);
            let update = on(rpm.clone(), "/usr/bin/spotifast", &suse).unwrap();
            assert_eq!(update.manager, Manager::Zypper);
            assert_eq!(
                update.command.as_deref(),
                Some("sudo zypper refresh && sudo zypper update spotifast")
            );
        }
    }

    #[test]
    fn homebrew_names_the_formula_or_the_cask_from_its_path() {
        let host = FakeHost::default();
        let formula = on(
            Unsupported::Homebrew,
            "/home/linuxbrew/.linuxbrew/Cellar/cww/0.11.1/bin/cww-app",
            &host,
        )
        .unwrap();
        assert_eq!(formula.manager, Manager::Homebrew);
        assert_eq!(formula.command.as_deref(), Some("brew upgrade cww"));
        let cask = on(
            Unsupported::Homebrew,
            "/opt/homebrew/Caskroom/zapfast/0.16.3/ZapFast.app/Contents/MacOS/zapfast",
            &host,
        )
        .unwrap();
        assert_eq!(cask.manager, Manager::HomebrewCask);
        assert_eq!(cask.command.as_deref(), Some("brew upgrade --cask zapfast"));
    }

    #[test]
    fn flatpaks_and_snaps_name_themselves() {
        let flatpak = FakeHost::default().with_var("FLATPAK_ID", "com.chatwithwork.desktop");
        assert_eq!(
            command(on(Unsupported::Flatpak, "/app/bin/cww-app", &flatpak)).as_deref(),
            Some("flatpak update com.chatwithwork.desktop")
        );
        let snap = FakeHost::default().with_var("SNAP_NAME", "zapfast");
        assert_eq!(
            command(on(Unsupported::Snap, "/snap/zapfast/12/bin/zapfast", &snap)).as_deref(),
            Some("sudo snap refresh zapfast")
        );
        // Without the name there's a manager but no command.
        let unnamed = on(
            Unsupported::Flatpak,
            "/app/bin/cww-app",
            &FakeHost::default(),
        )
        .unwrap();
        assert_eq!(unnamed.manager, Manager::Flatpak);
        assert_eq!(unnamed.command, None);
    }

    #[test]
    fn nix_finds_the_profile_element_holding_the_app() {
        let exe = "/nix/store/abc123-cww-0.11.1/bin/cww-app";
        let host = FakeHost::default().with_var("HOME", "/home/me");
        let system = on(Unsupported::Nix, exe, &host).unwrap();
        assert_eq!(system.manager, Manager::Nix);
        assert_eq!(system.command, None);
        let v3 = FakeHost::default().with_var("HOME", "/home/me").with_file(
            "/home/me/.nix-profile/manifest.json",
            r#"{"version":3,"elements":{"ripgrep":{"storePaths":["/nix/store/zzz-ripgrep"]},
               "chatwithwork-desktop":{"storePaths":["/nix/store/abc123-cww-0.11.1"]}}}"#,
        );
        assert_eq!(
            command(on(Unsupported::Nix, exe, &v3)).as_deref(),
            Some("nix profile upgrade chatwithwork-desktop")
        );
        // A stale ~/.nix-profile doesn't hide the XDG profile holding it.
        let v2 = FakeHost::default()
            .with_var("HOME", "/home/me")
            .with_file(
                "/home/me/.nix-profile/manifest.json",
                r#"{"version":3,"elements":{"other":{"storePaths":["/nix/store/zzz"]}}}"#,
            )
            .with_file(
            "/home/me/.local/state/nix/profile/manifest.json",
            r#"{"version":2,"elements":[{"storePaths":["/nix/store/zzz"]},{"storePaths":["/nix/store/abc123-cww-0.11.1"]}]}"#,
        );
        assert_eq!(
            command(on(Unsupported::Nix, exe, &v2)).as_deref(),
            Some("nix profile upgrade 1")
        );
    }

    #[test]
    fn cargo_reinstalls_the_crate_that_owns_the_binary() {
        let crates2 = r#"{"installs":{
            "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["rg"]},
            "zapfast 0.16.3 (git+https://github.com/crmne/zapfast?tag=v0.16.3#abc)":{"bins":["zapfast"]}}}"#;
        let host = FakeHost::default()
            .with_var("HOME", "/home/me")
            .with_file("/home/me/.cargo/.crates2.json", crates2);
        let update = on(Unsupported::Cargo, "/home/me/.cargo/bin/zapfast", &host).unwrap();
        assert_eq!(update.manager, Manager::Cargo);
        assert_eq!(update.package.as_deref(), Some("zapfast"));
        assert_eq!(
            update.command.as_deref(),
            Some("cargo install --git https://github.com/crmne/zapfast zapfast")
        );
        assert_eq!(
            command(on(Unsupported::Cargo, "/home/me/.cargo/bin/rg", &host)).as_deref(),
            Some("cargo install ripgrep")
        );
        // Another registry or SSH git: no command that would take crates.io's.
        let other = r#"{"installs":{
            "tool 1.0.0 (registry+https://my.registry/index)":{"bins":["tool"]},
            "app 1.0.0 (git+ssh://git@github.com/me/app#abc)":{"bins":["app"]}}}"#;
        let host = FakeHost::default()
            .with_var("HOME", "/home/me")
            .with_file("/home/me/.cargo/.crates2.json", other);
        for bin in ["tool", "app"] {
            let update = on(
                Unsupported::Cargo,
                &format!("/home/me/.cargo/bin/{bin}"),
                &host,
            )
            .unwrap();
            assert!(update.package.is_some());
            assert_eq!(update.command, None, "{bin}");
        }
    }

    #[test]
    fn copies_no_package_manager_owns_have_no_managed_update() {
        let host = FakeHost::default();
        for reason in [
            Unsupported::NotPortable,
            Unsupported::SystemDirectory,
            Unsupported::MoveToApplications,
            Unsupported::Platform,
        ] {
            assert_eq!(on(reason, "/usr/bin/zapfast", &host), None);
        }
    }

    #[test]
    fn plain_names_only() {
        assert!(plain("chatwithwork-desktop-bin"));
        assert!(plain("com.chatwithwork.desktop"));
        assert!(plain("libfoo+bar@1"));
        for bad in ["", "-rf", "a b", "a;b", "$(x)", "a'b"] {
            assert!(!plain(bad), "{bad}");
        }
    }
}
