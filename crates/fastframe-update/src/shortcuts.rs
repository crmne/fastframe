//! Pointing the user's launchers at an executable's new name.
//!
//! When an update moves an app off a legacy name, the launchers that name the
//! old path exactly are repointed: desktop entries and command links on
//! Linux, shortcuts in the Start menu, on the desktop, in the Startup folder
//! and pinned to the taskbar on Windows. Scripts and other files are the
//! user's own and are left alone. Nothing here fails an update: a launcher
//! that cannot be rewritten keeps its old target.

use std::fs;
use std::path::{Path, PathBuf};

use crate::detect::Platform;
use crate::host::Host;

/// Repoints the launchers that start `old` to start `new` instead.
pub(crate) fn repoint(host: &dyn Host, platform: Platform, old: &Path, new: &Path) {
    match platform {
        Platform::Linux => {
            let home = host.var("HOME").map(PathBuf::from);
            let under = |variable: &str, default: &str, rest: &str| {
                host.var(variable)
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .or_else(|| home.as_ref().map(|home| home.join(default)))
                    .map(|base| base.join(rest))
            };
            let entries = [
                under("XDG_DATA_HOME", ".local/share", "applications"),
                under("XDG_CONFIG_HOME", ".config", "autostart"),
                home.as_ref().map(|home| home.join("Desktop")),
            ];
            for folder in entries.into_iter().flatten() {
                repoint_desktop_entries(&folder, old, new);
            }
            for folder in [".local/bin", "bin"] {
                if let Some(home) = &home {
                    repoint_links(&home.join(folder), old, new);
                }
            }
        }
        Platform::Windows => {
            let folders: Vec<PathBuf> = [
                host.var("APPDATA").map(|roaming| {
                    // The Start menu, with the Startup folder inside it.
                    PathBuf::from(roaming).join(r"Microsoft\Windows\Start Menu\Programs")
                }),
                host.var("APPDATA").map(|roaming| {
                    PathBuf::from(roaming).join(r"Microsoft\Internet Explorer\Quick Launch")
                }),
                host.var("USERPROFILE")
                    .map(|profile| PathBuf::from(profile).join("Desktop")),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !folders.is_empty() {
                let _ = host.repoint_windows_shortcuts(&folders, old, new);
            }
        }
        Platform::MacOs | Platform::Other => {}
    }
}

/// Rewrites the `.desktop` files in `folder` and below that start `old`.
fn repoint_desktop_entries(folder: &Path, old: &Path, new: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            repoint_desktop_entries(&path, old, new);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "desktop")
            && let Ok(contents) = fs::read_to_string(&path)
            && let Some(rewritten) = repoint_desktop_entry(&contents, old, new)
        {
            let _ = replace_file(&path, rewritten.as_bytes());
        }
    }
}

/// `contents` with every `Exec` and `TryExec` that runs `old` running `new`,
/// or `None` when none does. Only a program written as the exact path, bare
/// or in double quotes, is recognised; anything with escapes is left alone.
pub(crate) fn repoint_desktop_entry(contents: &str, old: &Path, new: &Path) -> Option<String> {
    let (old, new) = (old.to_str()?, new.to_str()?);
    if old.contains(['\\', '"', '`', '$']) || new.contains(['\\', '"', '`', '$']) {
        return None;
    }
    let mut changed = false;
    let lines: Vec<String> = contents
        .split_inclusive('\n')
        .map(|line| {
            let (body, ending) = match line.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (line, ""),
            };
            let rewritten = if let Some(value) = body.strip_prefix("TryExec=") {
                (value.trim_end() == old).then(|| format!("TryExec={new}"))
            } else if let Some(value) = body.strip_prefix("Exec=") {
                let quoted = format!("\"{old}\"");
                [old, quoted.as_str()]
                    .into_iter()
                    .find_map(|program| {
                        let rest = value.strip_prefix(program)?;
                        (rest.is_empty() || rest.starts_with([' ', '\t']))
                            .then_some((program, rest))
                    })
                    .and_then(|(program, rest)| {
                        let program = if program.starts_with('"') {
                            format!("\"{new}\"")
                        } else if new.contains([' ', '\t']) {
                            return None;
                        } else {
                            new.to_owned()
                        };
                        Some(format!("Exec={program}{rest}"))
                    })
            } else {
                None
            };
            match rewritten {
                Some(rewritten) => {
                    changed = true;
                    rewritten + ending
                }
                None => line.to_owned(),
            }
        })
        .collect();
    changed.then(|| lines.concat())
}

/// Points the symbolic links in `folder` whose target is `old` at `new`.
#[cfg(unix)]
fn repoint_links(folder: &Path, old: &Path, new: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let link = entry.path();
        let Ok(target) = fs::read_link(&link) else {
            continue;
        };
        if folder.join(&target) != old {
            continue;
        }
        let Some(name) = link.file_name() else {
            continue;
        };
        let mut temporary = name.to_owned();
        temporary.push(".fastframe-update");
        let temporary = folder.join(temporary);
        let _ = fs::remove_file(&temporary);
        if std::os::unix::fs::symlink(new, &temporary).is_ok()
            && fs::rename(&temporary, &link).is_err()
        {
            let _ = fs::remove_file(&temporary);
        }
    }
}

#[cfg(not(unix))]
fn repoint_links(_folder: &Path, _old: &Path, _new: &Path) {}

/// Replaces `path` with `contents` through a sibling file, keeping its
/// permissions (a trusted desktop launcher is executable).
fn replace_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let permissions = fs::metadata(path)?.permissions();
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".fastframe-update");
    let temporary = PathBuf::from(temporary);
    let written = fs::write(&temporary, contents)
        .and_then(|()| fs::set_permissions(&temporary, permissions))
        .and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// The PowerShell that repoints Windows shortcuts. It reads its paths from
/// the environment, so no path is ever parsed as script.
pub(crate) const WINDOWS_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$old = $env:FASTFRAME_OLD
$new = $env:FASTFRAME_NEW
$shell = New-Object -ComObject WScript.Shell
foreach ($folder in ($env:FASTFRAME_FOLDERS -split "`n")) {
    if (-not $folder -or -not (Test-Path -LiteralPath $folder)) { continue }
    Get-ChildItem -LiteralPath $folder -Filter '*.lnk' -Recurse -File | ForEach-Object {
        $link = $shell.CreateShortcut($_.FullName)
        if ([string]::Equals($link.TargetPath, $old, [StringComparison]::OrdinalIgnoreCase)) {
            $link.TargetPath = $new
            if ($link.IconLocation.StartsWith($old + ',', [StringComparison]::OrdinalIgnoreCase)) {
                $link.IconLocation = $new + $link.IconLocation.Substring($old.Length)
            }
            $link.Save()
        }
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeHost;

    #[test]
    fn a_desktop_entry_runs_the_new_name() {
        let old = Path::new("/home/me/apps/fastpotify/fastpotify");
        let new = Path::new("/home/me/apps/fastpotify/spotifast");
        let entry = "[Desktop Entry]\nName=Fastpotify\nTryExec=/home/me/apps/fastpotify/fastpotify\nExec=/home/me/apps/fastpotify/fastpotify %U\nIcon=fastpotify\n\n[Desktop Action Play]\nExec=\"/home/me/apps/fastpotify/fastpotify\" play\n";
        assert_eq!(
            repoint_desktop_entry(entry, old, new).unwrap(),
            "[Desktop Entry]\nName=Fastpotify\nTryExec=/home/me/apps/fastpotify/spotifast\nExec=/home/me/apps/fastpotify/spotifast %U\nIcon=fastpotify\n\n[Desktop Action Play]\nExec=\"/home/me/apps/fastpotify/spotifast\" play\n"
        );
    }

    #[test]
    fn other_programs_and_longer_paths_are_left_alone() {
        let old = Path::new("/opt/fastpotify/fastpotify");
        let new = Path::new("/opt/fastpotify/spotifast");
        for entry in [
            "Exec=/opt/fastpotify/fastpotify-helper\n",
            "Exec=env FOO=1 /opt/fastpotify/fastpotify\n",
            "Exec=sh -c \"/opt/fastpotify/fastpotify\"\n",
            "Name=/opt/fastpotify/fastpotify\n",
        ] {
            assert_eq!(repoint_desktop_entry(entry, old, new), None, "{entry}");
        }
    }

    #[test]
    fn a_path_with_spaces_keeps_its_quotes() {
        let old = Path::new("/home/me/My Apps/fastpotify");
        let new = Path::new("/home/me/My Apps/spotifast");
        assert_eq!(
            repoint_desktop_entry("Exec=\"/home/me/My Apps/fastpotify\"\n", old, new).unwrap(),
            "Exec=\"/home/me/My Apps/spotifast\"\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn linux_launchers_in_the_home_folder_are_repointed() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let apps = home.join("apps");
        fs::create_dir_all(&apps).unwrap();
        let old = apps.join("fastpotify");
        let new = apps.join("spotifast");
        fs::write(&new, b"app").unwrap();
        let applications = home.join(".local/share/applications/sub");
        fs::create_dir_all(&applications).unwrap();
        let entry = applications.join("fastpotify.desktop");
        fs::write(
            &entry,
            format!("[Desktop Entry]\nExec={} %U\n", old.display()),
        )
        .unwrap();
        let untouched = applications.join("other.desktop");
        fs::write(&untouched, "[Desktop Entry]\nExec=/usr/bin/other\n").unwrap();
        let autostart = home.join("config/autostart");
        fs::create_dir_all(&autostart).unwrap();
        fs::write(
            autostart.join("fastpotify.desktop"),
            format!("Exec={} --hidden\n", old.display()),
        )
        .unwrap();
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(&old, bin.join("fastpotify")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/true", bin.join("other")).unwrap();

        let host = FakeHost::default()
            .with_var("HOME", home.as_os_str())
            .with_var("XDG_CONFIG_HOME", home.join("config").as_os_str());
        repoint(&host, Platform::Linux, &old, &new);

        assert_eq!(
            fs::read_to_string(&entry).unwrap(),
            format!("[Desktop Entry]\nExec={} %U\n", new.display())
        );
        assert_eq!(
            fs::read_to_string(autostart.join("fastpotify.desktop")).unwrap(),
            format!("Exec={} --hidden\n", new.display())
        );
        assert_eq!(
            fs::read_to_string(&untouched).unwrap(),
            "[Desktop Entry]\nExec=/usr/bin/other\n"
        );
        assert_eq!(fs::read_link(bin.join("fastpotify")).unwrap(), new);
        assert_eq!(
            fs::read_link(bin.join("other")).unwrap(),
            Path::new("/usr/bin/true")
        );
        assert!(host.repointed_shortcuts().is_empty());
    }

    #[test]
    fn windows_shortcuts_are_handed_to_the_system_with_their_folders() {
        let host = FakeHost::default()
            .with_var("APPDATA", r"C:\Users\me\AppData\Roaming")
            .with_var("USERPROFILE", r"C:\Users\me");
        let old = Path::new(r"C:\Programs\Spotifast\fastpotify.exe");
        let new = Path::new(r"C:\Programs\Spotifast\spotifast.exe");
        repoint(&host, Platform::Windows, old, new);
        let calls = host.repointed_shortcuts();
        assert_eq!(calls.len(), 1);
        let (folders, from, to) = &calls[0];
        assert_eq!((from.as_path(), to.as_path()), (old, new));
        assert_eq!(
            folders,
            &[
                PathBuf::from(r"C:\Users\me\AppData\Roaming")
                    .join(r"Microsoft\Windows\Start Menu\Programs"),
                PathBuf::from(r"C:\Users\me\AppData\Roaming")
                    .join(r"Microsoft\Internet Explorer\Quick Launch"),
                PathBuf::from(r"C:\Users\me").join("Desktop"),
            ]
        );
        // Without its folders, nothing is asked.
        let bare = FakeHost::default();
        repoint(&bare, Platform::Windows, old, new);
        assert!(bare.repointed_shortcuts().is_empty());
    }

    #[test]
    fn the_windows_script_takes_its_paths_from_the_environment() {
        for variable in [
            "$env:FASTFRAME_OLD",
            "$env:FASTFRAME_NEW",
            "$env:FASTFRAME_FOLDERS",
        ] {
            assert!(WINDOWS_SCRIPT.contains(variable), "{variable}");
        }
    }
}
