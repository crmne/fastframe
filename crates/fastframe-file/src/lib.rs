//! Replaces a file on disk without ever leaving it half written.
//!
//! Settings, sessions and credentials are rewritten whole. Writing over the
//! file in place leaves it truncated if the app is killed or the power goes
//! halfway through, and the app then starts from defaults. [`write()`] writes a
//! temporary file beside the target instead, flushes it to the disk, and moves
//! it over the target in one step, so a reader sees the old contents or the
//! new ones, never part of either. When the path is a symbolic link (a
//! dotfiles manager's, say), the link stays and the file it leads to is
//! replaced.
//!
//! Flushing to the disk can take tens of milliseconds, and seconds on a busy
//! or network drive. The thread that draws the window must never wait for
//! it: hand the bytes to a [`Writer`], which writes them on a thread of its
//! own, and call [`mark_ui_thread`] once on the drawing thread, so a durable
//! write there is caught (a panic in a debug build, an error in the log in a
//! release one).
//!
//! ```no_run
//! # fn main() -> std::io::Result<()> {
//! let path = std::path::Path::new("/home/ada/.config/example/settings.json");
//! fastframe_file::write(path, br#"{"theme":"dark"}"#)?;
//! // A secret only the user may read (0600 on Unix):
//! fastframe_file::write_private(&path.with_file_name("token"), b"...")?;
//! // Or stream into the file, from a serializer say:
//! fastframe_file::write_with(path, fastframe_file::Access::Default, |file| {
//!     use std::io::Write as _;
//!     file.write_all(b"{}")
//! })?;
//! # Ok(())
//! # }
//! ```
//!
//! On the thread that draws the window:
//!
//! ```no_run
//! use fastframe_file::{Access, Writer};
//! # let path = std::path::Path::new("settings.json");
//! fastframe_file::mark_ui_thread();
//! let files = Writer::new();
//! // Returns at once; a newer save of the same file replaces this one if it
//! // has not started yet.
//! files.write(path, br#"{"theme":"dark"}"#.to_vec(), Access::Default);
//! // Before the app exits:
//! files.flush(fastframe_file::FLUSH_TIMEOUT);
//! ```

use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
mod windows;
mod writer;

pub use writer::{FLUSH_TIMEOUT, Writer};

thread_local! {
    static UI_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Marks the calling thread as the one that draws the window. From then on
/// [`write()`], [`write_private`] and [`write_with`] refuse it: a debug
/// build panics, and a release build logs an error and still writes, rather
/// than lose what was saved. Hand its saves to a [`Writer`] instead.
pub fn mark_ui_thread() {
    UI_THREAD.with(|ui| ui.set(true));
}

/// Who may read the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    /// The system's default for a new file.
    #[default]
    Default,
    /// Only the user (mode 0600 on Unix). Windows keeps a user's profile
    /// private already, so there it is the same as `Default`.
    Private,
}

/// Replaces `path` with `contents`, creating its directory if needed.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    write_with(path, Access::Default, |file| {
        file.write_all(contents.as_ref())
    })
}

/// Replaces `path` with `contents` that only the user may read.
pub fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    write_with(path, Access::Private, |file| {
        file.write_all(contents.as_ref())
    })
}

/// Replaces `path` with what `fill` writes into a fresh file. If `fill` or
/// anything after it fails, `path` keeps its old contents and the temporary
/// file is removed.
pub fn write_with(
    path: &Path,
    access: Access,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    if UI_THREAD.with(Cell::get) {
        let message = format!(
            "{} was written on the UI thread, which then waits for the disk; \
             hand it to a fastframe_file::Writer",
            path.display()
        );
        if cfg!(debug_assertions) {
            panic!("{message}");
        }
        log::error!("{message}");
    }
    replace_with(path, access, fill)
}

/// [`write_with`] without the UI thread check, for the [`Writer`].
fn replace_with(
    path: &Path,
    access: Access,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    // A link at `path` stays a link: the file it leads to is replaced.
    let path = &follow_links(path)?;
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = create(&temporary, access)?;
        fill(&mut file)?;
        file.flush()?;
        // The contents reach the disk before the move, or a crash just after
        // it could leave the new name pointing at nothing.
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// How many symbolic links [`follow_links`] follows before it gives up, as
/// Linux does.
const MAX_LINKS: usize = 40;

/// The file `path` names once the symbolic links at its end are followed,
/// or `path` itself when it is not a link. The file need not exist.
///
/// A dotfiles manager keeps the settings in its repository and leaves a link
/// where the app looks. Moving the new file over the link would put a plain
/// file in its place, and the copy the user keeps would stop being the one
/// the app uses. Writing beside the file the link leads to keeps the move
/// within one directory, and so in one step. Links among the directories on
/// the way are the system's to follow.
///
/// More links than [`MAX_LINKS`], as a link that leads back to itself has,
/// is an error, and so is a path that cannot be looked at: nothing is
/// written then.
fn follow_links(path: &Path) -> io::Result<PathBuf> {
    let mut file = path.to_path_buf();
    for _ in 0..=MAX_LINKS {
        match file.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() => {}
            Ok(_) => return Ok(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(file),
            Err(error) => return Err(error),
        }
        // A relative target starts at the link's directory; an absolute one
        // replaces it.
        let target = std::fs::read_link(&file)?;
        file = match file.parent() {
            Some(dir) => dir.join(target),
            None => target,
        };
    }
    Err(io::Error::other(format!(
        "{} leads through more than {MAX_LINKS} symbolic links",
        path.display()
    )))
}

/// A name beside `path` that no other write, in this process or another,
/// uses at the same time: `.<name>.<pid>.<n>.tmp`. Hidden, so a directory
/// listing does not show it while it lives.
fn temporary_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

fn create(path: &Path, access: Access) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if access == Access::Private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = access;
    options.open(path)
}

/// Moves `temporary` over `path` in one step.
#[cfg(not(windows))]
fn replace(temporary: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(temporary, path)
}

#[cfg(windows)]
fn replace(temporary: &Path, path: &Path) -> io::Result<()> {
    windows::replace(temporary, path)
}

/// Makes the move itself durable: on Unix a rename lives in the directory,
/// which has to reach the disk too. Windows' write-through move covers it.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn a_file_is_written_and_then_replaced_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        write(&path, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        write(&path, "second, longer").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second, longer");
        write(&path, "3").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "3", "no tail left");
        assert!(leftovers(path.parent().unwrap()).is_empty());
    }

    #[test]
    fn a_failed_write_keeps_the_old_contents_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        write(&path, "kept").unwrap();
        let failed = write_with(&path, Access::Default, |file| {
            file.write_all(b"half")?;
            Err(io::Error::other("the serializer gave up"))
        });
        assert_eq!(failed.unwrap_err().to_string(), "the serializer gave up");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn writes_at_the_same_time_never_share_a_temporary_file() {
        let path = Path::new("/x/settings.json");
        let (a, b) = (temporary_path(path), temporary_path(path));
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".settings.json.") && name.ends_with(".tmp"));
        assert_eq!(a.parent(), path.parent(), "beside the file, on its disk");
    }

    #[test]
    fn a_bare_file_name_is_written_in_the_current_directory() {
        assert_eq!(temporary_path(Path::new("x")).parent(), Some(Path::new("")));
    }

    /// A durable write on the UI thread fails tests, so a save that would
    /// freeze the window cannot come back unnoticed.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "UI thread")]
    fn the_ui_thread_cannot_wait_for_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        mark_ui_thread();
        let _ = write(&dir.path().join("settings.json"), "{}");
    }

    /// A release build logs the mistake but still writes, rather than lose
    /// what was saved.
    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_build_still_writes_on_the_ui_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        mark_ui_thread();
        write(&path, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_readable_by_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        write_private(&path, "secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// A dotfiles manager's link stays, and the file in its repository is
    /// the one that changes.
    #[cfg(unix)]
    #[test]
    fn a_link_stays_and_the_file_it_leads_to_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("dotfiles/settings.json");
        std::fs::create_dir(kept.parent().unwrap()).unwrap();
        std::fs::write(&kept, "old").unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir(&config).unwrap();
        let link = config.join("settings.json");
        std::os::unix::fs::symlink("../dotfiles/settings.json", &link).unwrap();

        write(&link, "new").unwrap();

        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "new");
        assert!(leftovers(&config).is_empty());
        assert!(leftovers(kept.parent().unwrap()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_chain_of_links_is_followed_to_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        let middle = dir.path().join("middle");
        let first = dir.path().join("first");
        std::os::unix::fs::symlink(&file, &middle).unwrap();
        std::os::unix::fs::symlink("middle", &first).unwrap();

        write(&first, "through two links").unwrap();

        assert!(first.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(middle.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "through two links");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_that_leads_back_to_itself_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();

        let error = write(&a, "lost").unwrap_err();

        assert!(error.to_string().contains("symbolic links"), "{error}");
        assert!(a.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_path_that_is_not_a_link_is_written_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/settings.json");
        assert_eq!(follow_links(&path).unwrap(), path);
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(follow_links(&path).unwrap(), path);
    }
}
