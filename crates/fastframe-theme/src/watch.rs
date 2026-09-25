//! Filesystem notifications that wake the catalogue, so following a theme
//! needs no repaint timer.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::Waker;

/// Watches the themes directory, and Omarchy's current theme if followed.
pub(crate) struct ThemeWatch {
    _watcher: RecommendedWatcher,
    changed: Arc<AtomicBool>,
}

impl ThemeWatch {
    /// Starts watching. `waker` runs once per burst of changes, until
    /// [`Self::take_changed`] reads them.
    pub(crate) fn new(local: &Path, system: Option<&Path>, waker: &Waker) -> notify::Result<Self> {
        let changed = Arc::new(AtomicBool::new(false));
        let signal = changed.clone();
        let wake = waker.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let changed = event.is_ok_and(|event| {
                    event.kind.is_create() || event.kind.is_modify() || event.kind.is_remove()
                });
                if changed && !signal.swap(true, Ordering::AcqRel) {
                    wake.wake();
                }
            })?;
        watcher.watch(local, RecursiveMode::NonRecursive)?;
        if let Some(system) = system {
            // Omarchy replaces the whole `theme` directory on a switch.
            watcher.watch(system, RecursiveMode::Recursive)?;
        }
        Ok(Self {
            _watcher: watcher,
            changed,
        })
    }

    /// Whether anything changed since the last call.
    pub(crate) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};

    #[test]
    fn a_palette_replacement_signals_once_without_idle_notifications() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("themes");
        let system = directory.path().join("current");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&system).unwrap();
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = wakes.clone();
        let waker = Waker::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let watch = ThemeWatch::new(&local, Some(&system), &waker).unwrap();
        assert!(!watch.take_changed());
        let temporary = directory.path().join("next");
        fs::write(&temporary, "{}").unwrap();
        fs::rename(temporary, system.join("colors.toml")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !watch.take_changed() {
            assert!(Instant::now() < deadline, "the change was not delivered");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(wakes.load(Ordering::Relaxed) >= 1);
        assert!(!watch.take_changed());
    }
}
