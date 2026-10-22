//! How the app and its streams are named in the system's volume mixer.

use std::sync::OnceLock;

/// The app as the system's volume mixer names it: `pavucontrol`, GNOME's
/// and KDE's sound settings, `pw-dump` and `pactl` on Linux.
///
/// Set it once, at startup, with [`set_app_info`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AppInfo {
    /// The app's name as users know it, such as `"Spotifast"`.
    pub name: String,
    /// The app's icon, by its freedesktop icon name (usually the app id
    /// of its `.desktop` file), such as `"spotifast"`.
    pub icon: Option<String>,
}

/// What kind of sound a stream carries. The sound server uses it to route
/// the stream or to lower other streams, for example during a call, and
/// remembers the volume of a stream with a role by its role, so every app
/// playing [`StreamRole::Music`] shares one remembered volume. Without a
/// role, the volume is remembered for the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StreamRole {
    /// Music playback, such as Spotifast's.
    Music,
    /// The sound of a video or film.
    Movie,
    /// Game sound.
    Game,
    /// Calls and other two-way voice, such as ZapFast's calls.
    Communication,
    /// Notifications, alerts and other short sounds.
    Notification,
    /// Music production and other professional audio work, such as Solco's.
    Production,
    /// Screen readers and other accessibility output.
    Accessibility,
}

static APP: OnceLock<AppInfo> = OnceLock::new();

/// Names the app in the system's volume mixer, with its icon, for every
/// output and input the process opens afterwards (and for the connection it
/// keeps to PipeWire's PulseAudio server).
///
/// Call it once, at startup, before opening or listing anything. It can be
/// set only once; a second call returns the info it was given, unchanged.
/// Without it, the mixer shows the process name on PipeWire and
/// `cpal-pulseaudio-<pid>` on PulseAudio. It names the app on PipeWire and
/// PulseAudio; ALSA, CoreAudio and WASAPI have no place for it (WASAPI and
/// CoreAudio show the executable's own name and icon).
pub fn set_app_info(info: AppInfo) -> Result<(), AppInfo> {
    APP.set(info)
}

/// What [`set_app_info`] set, if it has been called.
pub(crate) fn app_info() -> Option<&'static AppInfo> {
    APP.get()
}

/// What a stream is called and what it carries, resolved from the app's
/// info and the stream's options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Naming {
    /// The app's name and icon.
    pub(crate) app: Option<AppInfo>,
    /// The stream's own name: the one the options give, or else the app's.
    pub(crate) media_name: Option<String>,
    /// What the stream carries.
    pub(crate) role: Option<StreamRole>,
}

impl Naming {
    /// The stream's naming: its own name if the options give one, the app's
    /// name otherwise, so that neither shows cpal's `cpal-playback-<pid>`.
    pub(crate) fn resolve(
        app: Option<&AppInfo>,
        media_name: Option<&str>,
        role: Option<StreamRole>,
    ) -> Self {
        let media_name = media_name
            .map(str::to_owned)
            .or_else(|| app.map(|app| app.name.clone()));
        Self {
            app: app.cloned(),
            media_name,
            role,
        }
    }

    /// Whether anything is named, so a host has something to apply.
    #[cfg_attr(
        not(any(target_os = "linux", test)),
        expect(dead_code, reason = "names only Linux streams")
    )]
    pub(crate) fn is_empty(&self) -> bool {
        self.app.is_none() && self.media_name.is_none() && self.role.is_none()
    }
}

/// `value` without NUL bytes, which the sound servers cannot take.
#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(dead_code, reason = "names only Linux streams")
)]
pub(crate) fn clean(value: &str) -> String {
    value.replace('\0', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppInfo {
        AppInfo {
            name: "Spotifast".into(),
            icon: Some("spotifast".into()),
        }
    }

    #[test]
    fn nul_bytes_are_dropped() {
        assert_eq!(clean("Spoti\0fast"), "Spotifast");
    }

    #[test]
    fn nothing_set_names_nothing() {
        let naming = Naming::resolve(None, None, None);
        assert!(naming.is_empty());
        assert_eq!(naming, Naming::default());
    }

    #[test]
    fn a_stream_without_a_name_takes_the_apps() {
        let naming = Naming::resolve(Some(&app()), None, Some(StreamRole::Music));
        assert_eq!(naming.app, Some(app()));
        assert_eq!(naming.media_name.as_deref(), Some("Spotifast"));
        assert_eq!(naming.role, Some(StreamRole::Music));
        assert!(!naming.is_empty());
    }

    #[test]
    fn a_stream_name_wins_over_the_apps() {
        let naming = Naming::resolve(Some(&app()), Some("Voice call"), None);
        assert_eq!(naming.media_name.as_deref(), Some("Voice call"));
        let naming = Naming::resolve(None, Some("Voice call"), None);
        assert_eq!(naming.app, None);
        assert_eq!(naming.media_name.as_deref(), Some("Voice call"));
    }
}
