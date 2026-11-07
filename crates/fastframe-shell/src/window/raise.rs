//! Bring the existing window forward through the compositor.
//!
//! A Wayland client cannot focus or unminimize its own window: the
//! compositor drops the request. An app that wants its window back (a
//! notification click, a second launch) then closes the window and opens a
//! new one, which lands on the current workspace instead of taking the user
//! to the window where it was. niri can focus a window on request through
//! its command line tool, which switches to its workspace, so nothing is
//! closed or recreated (ZapFast #474).
//!
//! Hyprland is not asked: its `hyprctl dispatch` syntax changed to Lua in
//! 0.56, so the same request fails on one version or the other.

/// Asks the compositor to focus this process's window, switching to its
/// workspace. Returns whether it did.
///
/// Under niri (`NIRI_SOCKET` set) this runs `niri msg --json windows` to
/// find the window by process id, then `niri msg action focus-window --id`,
/// and waits for them, which takes a few milliseconds. `false` on any other
/// compositor or platform, or when `niri` is missing (a Flatpak sandbox) or
/// fails, so the app falls back to what it did before: on Wayland, closing
/// the window and opening a new one.
pub fn raise_on_compositor() -> bool {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        under_niri(std::env::var_os("NIRI_SOCKET")) && linux::focus_on_niri(std::process::id())
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        false
    }
}

/// Whether `NIRI_SOCKET` names niri's socket. An empty value counts as
/// unset.
#[cfg(any(any(target_os = "linux", target_os = "freebsd"), test))]
fn under_niri(socket: Option<std::ffi::OsString>) -> bool {
    socket.is_some_and(|socket| !socket.is_empty())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux {
    use std::process::{Command, Stdio};

    pub(super) fn focus_on_niri(pid: u32) -> bool {
        let Ok(windows) = Command::new("niri")
            .args(["msg", "--json", "windows"])
            .stderr(Stdio::null())
            .output()
        else {
            return false;
        };
        let Some(id) = windows
            .status
            .success()
            .then(|| super::niri_window_id(&windows.stdout, pid))
            .flatten()
        else {
            return false;
        };
        Command::new("niri")
            .args(["msg", "action", "focus-window", "--id", &id.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// The id of the window owned by process `pid` in `niri msg --json windows`.
#[cfg(any(any(target_os = "linux", target_os = "freebsd"), test))]
fn niri_window_id(json: &[u8], pid: u32) -> Option<u64> {
    let windows: serde_json::Value = serde_json::from_slice(json).ok()?;
    windows
        .as_array()?
        .iter()
        .find(|window| window["pid"].as_u64() == Some(u64::from(pid)))?["id"]
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niri_window_id_finds_the_window_of_this_process() {
        let json = br#"[{"id":7,"pid":100},{"id":226,"pid":16363},{"id":9,"pid":null}]"#;
        assert_eq!(niri_window_id(json, 16363), Some(226));
        assert_eq!(niri_window_id(json, 1), None);
        assert_eq!(niri_window_id(b"not json", 1), None);
        assert_eq!(niri_window_id(br#"{"Err":"no"}"#, 100), None);
    }

    /// A window as niri prints it, with the fields around the two read.
    #[test]
    fn niri_window_id_reads_a_full_window() {
        let json = br#"[{"id":12,"title":"ZapFast","app_id":"zapfast","pid":4242,
            "workspace_id":3,"is_focused":false,"is_floating":false,"is_urgent":false}]"#;
        assert_eq!(niri_window_id(json, 4242), Some(12));
    }

    #[test]
    fn niri_is_found_by_its_socket() {
        assert!(under_niri(Some("/run/user/1000/niri.sock".into())));
        assert!(!under_niri(Some("".into())));
        assert!(!under_niri(None));
    }
}
