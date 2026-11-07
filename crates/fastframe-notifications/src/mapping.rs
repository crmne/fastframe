#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "windows", test))]
use crate::App;
#[cfg(any(target_os = "windows", test))]
use crate::{Error, Notification, Permission, Sound};
#[cfg(any(target_os = "windows", test))]
use sha2::{Digest as _, Sha256};
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_tags(key: &str) -> (String, String) {
    let hash: String = Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (hash[..16].into(), hash[16..32].into())
}
pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_xml(value: &Notification) -> String {
    let image = value
        .image
        .as_ref()
        .and_then(|path| url::Url::from_file_path(path).ok())
        .map(|url| {
            format!(
                "<image placement=\"appLogoOverride\" hint-crop=\"circle\" src=\"{}\"/>",
                escape(url.as_str())
            )
        })
        .unwrap_or_default();
    let actions: String = value
        .actions
        .iter()
        .filter(|(id, _)| id != "default")
        .map(|(id, label)| {
            format!(
                "<action content=\"{}\" arguments=\"{}\" activationType=\"foreground\"/>",
                escape(label),
                escape(id)
            )
        })
        .collect();
    let sound = if value.sound == Sound::Silent {
        "<audio silent=\"true\"/>"
    } else {
        "<audio src=\"ms-winsoundevent:Notification.Default\"/>"
    };
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text>{image}</binding></visual><actions>{actions}</actions>{sound}</toast>",
        escape(&value.title),
        escape(&value.body)
    )
}

/// Values of the app's `AppUserModelId` registry key. `IconUri` names the
/// logo tile Windows shows on the app's toasts; without it they show a blank
/// icon. The tile is opaque, so `IconBackgroundColor` stays unset (setting it
/// did not stop the dark theme from inverting a monochrome icon).
#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_identity(app: &App) -> Vec<(&'static str, String)> {
    let mut values = vec![("DisplayName", app.name.clone())];
    if let Some(path) = &app.icon_file {
        values.push(("IconUri", path.to_string_lossy().into_owned()));
    }
    values
}

/// Permission from `ToastNotifier.Setting`: whether it is enabled, or the
/// HRESULT it failed with. An AppUserModelID without a Start menu shortcut or
/// a package has no notification settings, and Windows reports
/// `ERROR_NOT_FOUND`. The user has turned nothing off, so a toast may be
/// shown; treating it as a failure kept every such app's toasts from showing.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_permission(setting: Result<bool, i32>) -> Result<Permission, Error> {
    /// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`.
    const ELEMENT_NOT_FOUND: i32 = 0x8007_0490_u32 as i32;
    match setting {
        Ok(true) => Ok(Permission::Granted),
        Ok(false) => Ok(Permission::Denied),
        Err(ELEMENT_NOT_FOUND) => Ok(Permission::NotDetermined),
        Err(_) => Err(Error::Delivery),
    }
}

/// The Freedesktop `app_icon`: the theme name while an installed icon carries
/// it, so packaged apps follow the theme, and the app's logo file otherwise.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub(crate) fn linux_app_icon(app: &App, installed: impl FnOnce(&str) -> bool) -> String {
    match &app.icon_file {
        Some(file)
            if !app.icon.starts_with('/')
                && !app.icon.starts_with("file://")
                && !installed(&app.icon) =>
        {
            file.to_string_lossy().into_owned()
        }
        _ => app.icon.clone(),
    }
}

/// Where an installed app puts its icon, in the icon theme specification's
/// order: `~/.icons`, then `icons` in each XDG data folder, then `pixmaps`.
#[cfg(any(target_os = "linux", target_os = "freebsd", all(unix, test)))]
pub(crate) fn linux_icon_folders(
    home: Option<PathBuf>,
    data_home: Option<PathBuf>,
    data_dirs: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let data_home = data_home
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let data_dirs: Vec<PathBuf> = data_dirs
        .filter(|dirs| !dirs.is_empty())
        .map(|dirs| std::env::split_paths(&dirs).collect())
        .unwrap_or_else(|| vec!["/usr/local/share".into(), "/usr/share".into()]);
    home.map(|home| home.join(".icons"))
        .into_iter()
        .chain(
            data_home
                .into_iter()
                .chain(data_dirs.into_iter().filter(|dir| dir.is_absolute()))
                .map(|dir| dir.join("icons")),
        )
        .chain(Some("/usr/share/pixmaps".into()))
        .collect()
}

/// Whether `name` is installed in the `hicolor` theme, which every icon theme
/// falls back to and where applications install their icons, or as a pixmap.
/// `subfolders` lists a folder's subfolders; `is_file` checks one path.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
pub(crate) fn linux_icon_installed(
    name: &str,
    folders: &[PathBuf],
    subfolders: impl Fn(&Path) -> Vec<PathBuf>,
    is_file: impl Fn(&Path) -> bool,
) -> bool {
    const EXTENSIONS: [&str; 3] = ["png", "svg", "xpm"];
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', '\0']) {
        return false;
    }
    let found = |folder: &Path| {
        EXTENSIONS
            .iter()
            .any(|extension| is_file(&folder.join(format!("{name}.{extension}"))))
    };
    folders.iter().any(|folder| {
        found(folder)
            || subfolders(&folder.join("hicolor"))
                .iter()
                .any(|size| found(&size.join("apps")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_app_without_windows_notification_settings_may_still_show_toasts() {
        assert_eq!(windows_permission(Ok(true)), Ok(Permission::Granted));
        assert_eq!(windows_permission(Ok(false)), Ok(Permission::Denied));
        assert_eq!(
            windows_permission(Err(0x8007_0490_u32 as i32)),
            Ok(Permission::NotDetermined)
        );
        assert_eq!(
            windows_permission(Err(0x8000_4005_u32 as i32)),
            Err(Error::Delivery)
        );
    }
    #[test]
    fn windows_identity_names_the_logo_only_when_given() {
        let app = App::new("com.example.app", "Example");
        assert_eq!(
            windows_identity(&app),
            vec![("DisplayName", "Example".to_owned())]
        );
        let logo = std::env::temp_dir().join("example.ico");
        assert_eq!(
            windows_identity(&App {
                icon_file: Some(logo.clone()),
                ..app
            }),
            vec![
                ("DisplayName", "Example".to_owned()),
                ("IconUri", logo.to_string_lossy().into_owned()),
            ]
        );
    }
    #[test]
    fn linux_icon_follows_the_theme_and_falls_back_to_the_logo() {
        let named = App::new("com.example.app", "Example").icon("example");
        assert_eq!(linux_app_icon(&named, |_| false), "example");
        let logo = std::env::temp_dir().join("example.png");
        let app = App {
            icon_file: Some(logo.clone()),
            ..named.clone()
        };
        assert_eq!(linux_app_icon(&app, |name| name == "example"), "example");
        assert_eq!(
            linux_app_icon(&app, |_| false),
            logo.to_string_lossy().into_owned()
        );
        let path = app.icon("/opt/example/icon.png");
        assert_eq!(linux_app_icon(&path, |_| false), "/opt/example/icon.png");
    }
    // Unix paths only: on Windows `/usr/share` is not absolute.
    #[cfg(unix)]
    #[test]
    fn linux_icon_folders_follow_the_icon_theme_specification() {
        assert_eq!(
            linux_icon_folders(Some("/home/a".into()), None, None),
            [
                "/home/a/.icons",
                "/home/a/.local/share/icons",
                "/usr/local/share/icons",
                "/usr/share/icons",
                "/usr/share/pixmaps",
            ]
            .map(PathBuf::from)
        );
        assert_eq!(
            linux_icon_folders(
                None,
                Some("/data".into()),
                Some("/flatpak/share:relative:/usr/share".into())
            ),
            [
                "/data/icons",
                "/flatpak/share/icons",
                "/usr/share/icons",
                "/usr/share/pixmaps"
            ]
            .map(PathBuf::from)
        );
    }
    #[test]
    fn linux_icon_is_found_in_hicolor_or_pixmaps_only() {
        let folders = [PathBuf::from("/share/icons"), PathBuf::from("/pixmaps")];
        let installed = |files: &[&str], name: &str| {
            linux_icon_installed(
                name,
                &folders,
                |folder| {
                    if folder == Path::new("/share/icons/hicolor") {
                        vec![folder.join("48x48"), folder.join("scalable")]
                    } else {
                        Vec::new()
                    }
                },
                |path| files.iter().any(|file| path == Path::new(file)),
            )
        };
        assert!(installed(
            &["/share/icons/hicolor/scalable/apps/example.svg"],
            "example"
        ));
        assert!(installed(&["/pixmaps/example.png"], "example"));
        assert!(!installed(
            &["/share/icons/Adwaita/48x48/apps/example.png"],
            "example"
        ));
        assert!(!installed(
            &["/share/icons/hicolor/48x48/apps/other.png"],
            "example"
        ));
        assert!(!installed(&["/share/icons/hicolor/48x48/apps/..png"], "."));
        assert!(!installed(
            &["/share/icons/hicolor/48x48/apps/a/b.png"],
            "a/b"
        ));
    }
    #[test]
    fn windows_payload_cannot_inject_xml_or_actions() {
        let payload = windows_xml(
            &Notification::new("A < B")
                .body("a & b")
                .action("open\"x", "<open>")
                .sound(Sound::Silent),
        );
        assert!(payload.contains("A &lt; B"));
        assert!(payload.contains("a &amp; b"));
        assert!(payload.contains("arguments=\"open&quot;x\""));
        assert!(payload.contains("content=\"&lt;open&gt;\""));
        assert!(payload.contains("<audio silent=\"true\"/>"));
    }
    #[test]
    fn stable_windows_ids_fit_both_native_limits() {
        let (tag, group) = windows_tags("account-1/chat-42/reply-3");
        assert_eq!(tag.len(), 16);
        assert_eq!(group.len(), 16);
        assert_eq!(
            (tag.clone(), group.clone()),
            windows_tags("account-1/chat-42/reply-3")
        );
        assert_ne!((tag, group), windows_tags("account-2/chat-42/reply-3"));
    }
}

/// Bounded native ID history. The Linux session owns one instance, so a new
/// notification server starts with an empty history even if it reuses IDs.
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
#[derive(Default)]
pub(crate) struct History(std::collections::VecDeque<(String, String, u32)>);
#[cfg(any(target_os = "linux", target_os = "freebsd", test))]
impl History {
    pub(crate) fn get(&self, app: &str, key: &str) -> Option<u32> {
        self.0
            .iter()
            .find(|(a, k, _)| a == app && k == key)
            .map(|(_, _, id)| *id)
    }
    pub(crate) fn remember(&mut self, app: &str, key: &str, id: u32) {
        self.0.retain(|(a, k, _)| a != app || k != key);
        self.0.push_back((app.into(), key.into(), id));
        while self.0.len() > 256 {
            self.0.pop_front();
        }
    }
    pub(crate) fn forget(&mut self, app: &str, key: &str, expected: u32) {
        self.0
            .retain(|(a, k, id)| a != app || k != key || *id != expected);
    }
}
#[cfg(test)]
mod history_tests {
    use super::*;
    #[test]
    fn retained_ids_are_bounded_and_scoped_to_app_and_generation() {
        let mut history = History::default();
        history.remember("one", "chat", 10);
        history.remember("two", "chat", 20);
        history.forget("one", "chat", 11);
        assert_eq!(history.get("one", "chat"), Some(10));
        history.forget("one", "chat", 10);
        assert_eq!(history.get("two", "chat"), Some(20));
        for id in 0..300 {
            history.remember("one", &id.to_string(), id);
        }
        assert_eq!(history.0.len(), 256);
        assert_eq!(history.get("two", "chat"), None);
        assert_eq!(history.get("one", "299"), Some(299));
    }
    #[test]
    fn plain_text_is_preserved_without_interpreting_markup() {
        assert_eq!(escape("<b>A & B</b>"), "&lt;b&gt;A &amp; B&lt;/b&gt;");
    }
}
