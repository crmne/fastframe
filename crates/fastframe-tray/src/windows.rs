//! The Windows notification-area item, on its own thread with a message loop.

// The Win32 message loop is FFI.
#![allow(unsafe_code)]

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP, WM_QUIT,
};

use crate::{Config, Router};

pub(crate) struct Host {
    labels: Sender<(String, String)>,
    thread_id: u32,
}

impl Host {
    /// Makes the item on a new thread and waits for it to exist.
    pub(crate) fn start(config: Config, router: Router) -> Result<Self, String> {
        let (labels, relabel) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name(format!("{}-tray", config.id))
            .spawn(move || serve(&config, router, &relabel, &ready_tx))
            .map_err(|error| error.to_string())?;
        let thread_id = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "the tray thread did not answer".to_owned())??;
        Ok(Self { labels, thread_id })
    }

    pub(crate) fn set_label(&mut self, id: &str, label: String) {
        if self.labels.send((id.to_owned(), label)).is_ok() {
            self.poke(WM_APP);
        }
    }

    /// The item runs on its own thread from the start.
    pub(crate) fn attach(&mut self) {}

    /// Posts `message` to the tray thread's loop.
    fn poke(&self, message: u32) {
        // SAFETY: posting to a thread id is valid even if the thread has
        // ended; the call then fails, which is harmless here.
        unsafe {
            PostThreadMessageW(self.thread_id, message, 0, 0);
        }
    }
}

impl Drop for Host {
    /// Ends the tray thread, which removes the item.
    fn drop(&mut self) {
        self.poke(WM_QUIT);
    }
}

/// The tray thread: makes the item, reports its thread id, and runs the
/// message loop until `WM_QUIT`.
fn serve(
    config: &Config,
    router: Router,
    relabel: &Receiver<(String, String)>,
    ready: &Sender<Result<u32, String>>,
) {
    let item = match crate::native::build(config, router) {
        Ok(item) => item,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    // SAFETY: no preconditions.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    // SAFETY: MSG is plain data; all-zero is a valid value to fill in.
    let mut message: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: `message` is a valid MSG for this thread's queue.
    while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        if message.message == WM_APP {
            for (id, label) in relabel.try_iter() {
                item.set_label(&id, &label);
            }
            continue;
        }
        // SAFETY: `message` was just filled in by GetMessageW.
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
