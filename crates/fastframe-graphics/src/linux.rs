//! The dialog through zenity or kdialog, whichever the desktop has. Without
//! either, the error stays in the log and on standard error.

use std::process::{Command, Stdio};

use crate::Dialog;

pub(crate) fn show(message: &Dialog) -> bool {
    let zenity = if message.link.is_some() {
        Command::new("zenity")
            .args(["--question", "--icon=dialog-error", "--width=440"])
            .arg(format!("--ok-label={}", message.open_label))
            .arg(format!("--cancel-label={}", message.close_label))
            .arg(format!("--title={}", message.title))
            .arg(format!("--text={}", escape_markup(&message.body)))
            .stdin(Stdio::null())
            .status()
    } else {
        Command::new("zenity")
            .args(["--error", "--width=440"])
            .arg(format!("--title={}", message.title))
            .arg(format!("--text={}", escape_markup(&message.body)))
            .stdin(Stdio::null())
            .status()
    };
    if let Ok(status) = zenity {
        // zenity exits 0 for the first button, 1 for the other or a close.
        return status.success() && message.link.is_some();
    }
    let kind = if message.link.is_some() {
        "--warningyesno"
    } else {
        "--error"
    };
    match Command::new("kdialog")
        .arg("--title")
        .arg(&message.title)
        .arg(kind)
        .arg(&message.body)
        .stdin(Stdio::null())
        .status()
    {
        Ok(status) => status.success() && message.link.is_some(),
        Err(_) => {
            log::warn!("neither zenity nor kdialog is installed to show the error");
            false
        }
    }
}

pub(crate) fn open_url(url: &str) {
    let browser = std::env::var("BROWSER").ok();
    let candidates = url_openers(browser.as_deref(), cfg!(target_os = "freebsd"));
    let opened = try_open_url(url, &candidates, |program, args| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .spawn()
            .map(|_| ())
    });
    if let Err(error) = opened {
        log::warn!("could not open the help page ({url}): {error}");
        #[cfg(target_os = "freebsd")]
        {
            let _ = show(&fallback_url_dialog(url));
        }
    }
}

/// Candidate commands for opening `url`. Linux keeps `xdg-open`; FreeBSD
/// falls back through `gio open`, `$BROWSER` and common desktop browsers when
/// `xdg-utils` is not installed.
fn url_openers(browser_env: Option<&str>, fallbacks: bool) -> Vec<(String, Vec<&'static str>)> {
    let mut commands = vec![("xdg-open".to_owned(), Vec::new())];
    if !fallbacks {
        return commands;
    }
    commands.push(("gio".to_owned(), vec!["open"]));
    if let Some(browser) = browser_env.map(str::trim).filter(|b| !b.is_empty()) {
        commands.push((browser.to_owned(), Vec::new()));
    }
    for program in ["kde-open", "firefox", "chromium", "chrome", "falkon", "epiphany"] {
        commands.push((program.to_owned(), Vec::new()));
    }
    commands
}

fn try_open_url<E>(
    url: &str,
    candidates: &[(String, Vec<&'static str>)],
    mut spawn: impl FnMut(&str, &[&str]) -> Result<(), E>,
) -> Result<(), E> {
    let mut last_error = None;
    for (program, prefix_args) in candidates {
        let mut args: Vec<&str> = prefix_args.clone();
        args.push(url);
        match spawn(program, &args) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if last_error.is_none() {
                    last_error = Some(error);
                }
            }
        }
    }
    Err(last_error.expect("at least one URL opener candidate"))
}

#[cfg(any(target_os = "freebsd", test))]
fn fallback_url_dialog(url: &str) -> Dialog {
    Dialog {
        title: "Help page".to_owned(),
        body: format!(
            "Could not open a web browser automatically.\n\nOpen this address in your browser:\n\n{url}"
        ),
        link: None,
        open_label: String::new(),
        close_label: "Close".to_owned(),
    }
}

/// zenity reads its text as Pango markup.
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_characters_in_the_details_stay_text() {
        assert_eq!(escape_markup("a < b & c"), "a &lt; b &amp; c");
    }

    #[test]
    fn freebsd_url_openers_fall_back_and_surface_the_recovery_url() {
        let candidates = url_openers(Some("custom-browser"), true);
        let mut tried = Vec::new();
        let opened = try_open_url("https://example.com/help", &candidates, |program, args| {
            tried.push((
                program.to_owned(),
                args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(),
            ));
            if program == "custom-browser" {
                Ok(())
            } else {
                Err("missing")
            }
        });
        assert!(opened.is_ok());
        assert_eq!(tried[0].0, "xdg-open");
        assert_eq!(
            tried[1],
            (
                "gio".to_owned(),
                vec!["open".to_owned(), "https://example.com/help".to_owned()],
            )
        );
        assert_eq!(
            tried[2],
            (
                "custom-browser".to_owned(),
                vec!["https://example.com/help".to_owned()],
            )
        );

        let dialog = fallback_url_dialog("https://example.com/help");
        assert!(dialog.body.contains("https://example.com/help"));
        assert_eq!(dialog.link, None);
    }
}
