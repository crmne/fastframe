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
    if let Err(error) = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .spawn()
    {
        log::warn!("could not open the help page: {error}");
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
    #[test]
    fn markup_characters_in_the_details_stay_text() {
        assert_eq!(super::escape_markup("a < b & c"), "a &lt; b &amp; c");
    }
}
