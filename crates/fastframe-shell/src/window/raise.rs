//! Bring the existing window forward through the compositor.
//!
//! A Wayland client cannot focus or unminimize its own window: the
//! compositor drops the request. An app that wants its window back (a
//! notification click, a second launch) then closes the window and opens a
//! new one, which lands on the current workspace instead of taking the user
//! to the window where it was. niri and Hyprland can focus a window on
//! request through their command line tools, which switches to its
//! workspace, so nothing is closed or recreated (ZapFast #474).
//!
//! Hyprland's `hyprctl dispatch` syntax changed to Lua in 0.56, so both
//! forms are sent, the Lua one first. Only a reply of `ok` counts: 0.56 exits
//! 0 with a warning when the window is gone, and rejects the old form.

/// Asks the compositor to focus this process's window, switching to its
/// workspace. Returns whether it did.
///
/// Under niri (`NIRI_SOCKET` set) this runs `niri msg --json windows` to
/// find the window by process id, then `niri msg action focus-window --id`.
/// Under Hyprland (`HYPRLAND_INSTANCE_SIGNATURE` set) it runs
/// `hyprctl clients -j`, then `hyprctl dispatch` in the Lua syntax of 0.56
/// and later, or in the older one if that is refused. It waits for them,
/// which takes a few milliseconds. `false` on any other compositor or
/// platform, or when the tool is missing (a Flatpak sandbox) or fails, so
/// the app falls back to what it did before: on Wayland, closing the window
/// and opening a new one.
pub fn raise_on_compositor() -> bool {
    #[cfg(target_os = "linux")]
    {
        let pid = std::process::id();
        if is_set(std::env::var_os("NIRI_SOCKET")) {
            linux::focus_on_niri(pid)
        } else if is_set(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")) {
            linux::focus_on_hyprland(pid)
        } else {
            false
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Whether a compositor's environment variable (`NIRI_SOCKET`,
/// `HYPRLAND_INSTANCE_SIGNATURE`) is set. An empty value counts as unset.
#[cfg(any(target_os = "linux", test))]
fn is_set(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

#[cfg(target_os = "linux")]
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

    pub(super) fn focus_on_hyprland(pid: u32) -> bool {
        let Ok(clients) = Command::new("hyprctl")
            .args(["clients", "-j"])
            .stderr(Stdio::null())
            .output()
        else {
            return false;
        };
        let Some(address) = clients
            .status
            .success()
            .then(|| super::hyprland_window_address(&clients.stdout, pid))
            .flatten()
        else {
            return false;
        };
        super::hyprland_focus_requests(&address)
            .iter()
            .any(|request| {
                Command::new("hyprctl")
                    .args(["dispatch", request])
                    .stderr(Stdio::null())
                    .output()
                    .is_ok_and(|reply| super::hyprland_dispatched(&reply.stdout))
            })
    }
}

/// The id of the window owned by process `pid` in `niri msg --json windows`.
#[cfg(any(target_os = "linux", test))]
fn niri_window_id(json: &[u8], pid: u32) -> Option<u64> {
    let windows: serde_json::Value = serde_json::from_slice(json).ok()?;
    windows
        .as_array()?
        .iter()
        .find(|window| window["pid"].as_u64() == Some(u64::from(pid)))?["id"]
        .as_u64()
}

/// The address of the window owned by process `pid` in `hyprctl clients -j`,
/// only when it is the hex number Hyprland prints, since it goes into a Lua
/// string.
#[cfg(any(target_os = "linux", test))]
fn hyprland_window_address(json: &[u8], pid: u32) -> Option<String> {
    let clients: serde_json::Value = serde_json::from_slice(json).ok()?;
    let address = clients
        .as_array()?
        .iter()
        .find(|client| client["pid"].as_u64() == Some(u64::from(pid)))?["address"]
        .as_str()?;
    let digits = address.strip_prefix("0x")?;
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| address.to_owned())
}

/// The `hyprctl dispatch` requests that focus the window at `address`: the
/// Lua syntax of Hyprland 0.56 and later, then the older one.
#[cfg(any(target_os = "linux", test))]
fn hyprland_focus_requests(address: &str) -> [String; 2] {
    [
        format!("hl.dsp.focus({{ window = \"address:{address}\" }})"),
        format!("focuswindow address:{address}"),
    ]
}

/// Whether Hyprland carried out a dispatch, which it answers with `ok`.
#[cfg(any(target_os = "linux", test))]
fn hyprland_dispatched(reply: &[u8]) -> bool {
    reply.trim_ascii() == b"ok"
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
    fn a_compositor_is_found_by_its_variable() {
        assert!(is_set(Some("/run/user/1000/niri.sock".into())));
        assert!(is_set(Some("efb50993_1793595380_1146209823".into())));
        assert!(!is_set(Some("".into())));
        assert!(!is_set(None));
    }

    #[test]
    fn hyprland_window_address_finds_the_window_of_this_process() {
        let json = br#"[{"address":"0x1","pid":100},{"address":"0x5556f889ce10","pid":16363},
            {"address":"0x2","pid":null}]"#;
        assert_eq!(
            hyprland_window_address(json, 16363).as_deref(),
            Some("0x5556f889ce10")
        );
        assert_eq!(hyprland_window_address(json, 1), None);
        assert_eq!(hyprland_window_address(b"not json", 1), None);
        assert_eq!(hyprland_window_address(b"[]", 100), None);
    }

    /// A client as Hyprland 0.56 prints it, with the fields around the two read.
    #[test]
    fn hyprland_window_address_reads_a_full_client() {
        let json = br#"[{"address":"0x5556f87d8c50","mapped":true,"hidden":false,"at":[2575,52],
            "size":[1250,1222],"workspace":{"id":3,"name":"3"},"floating":false,"monitor":1,
            "class":"zapfast","title":"ZapFast","initialClass":"zapfast","pid":1490936,
            "xwayland":false,"focusHistoryID":4}]"#;
        assert_eq!(
            hyprland_window_address(json, 1490936).as_deref(),
            Some("0x5556f87d8c50")
        );
    }

    /// The address is written into a Lua string, so only hex gets through.
    #[test]
    fn hyprland_window_address_refuses_anything_but_hex() {
        for address in [r#"0x1\" }) hl.exec(\"x"#, "0x", "1234", "0xabcg", ""] {
            let json = format!(r#"[{{"address":{},"pid":7}}]"#, serde_json::json!(address));
            assert_eq!(
                hyprland_window_address(json.as_bytes(), 7),
                None,
                "{address}"
            );
        }
        assert_eq!(
            hyprland_window_address(br#"[{"address":7,"pid":7}]"#, 7),
            None
        );
    }

    #[test]
    fn hyprland_focus_requests_try_lua_then_the_older_syntax() {
        assert_eq!(
            hyprland_focus_requests("0x5556f87d8c50"),
            [
                r#"hl.dsp.focus({ window = "address:0x5556f87d8c50" })"#.to_owned(),
                "focuswindow address:0x5556f87d8c50".to_owned(),
            ]
        );
    }

    /// Replies captured from `hyprctl dispatch` on Hyprland 0.56.2.
    #[test]
    fn only_ok_counts_as_dispatched() {
        assert!(hyprland_dispatched(b"ok\n"));
        assert!(hyprland_dispatched(b"ok"));
        // The Lua form, for a window that is gone: exit status 0.
        assert!(!hyprland_dispatched(
            b"warning: =[C]:-1: hl.focus: window not found\n"
        ));
        // The older form, which 0.56 refuses with exit status 7.
        assert!(!hyprland_dispatched(
            b"error: [string \"return hl.dispatch(focuswindow address:0x5556...\"]:1: ')' expected near 'address'\n"
        ));
        assert!(!hyprland_dispatched(b""));
    }
}
