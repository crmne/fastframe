//! The Windows controls' thread: a hidden window for them to belong to, COM,
//! and a message loop their callbacks arrive through.

// The window, COM and the message loop are FFI.
#![allow(unsafe_code)]

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostThreadMessageW,
    RegisterClassW, TranslateMessage, WM_APP, WNDCLASSW, WS_OVERLAPPED,
};

use crate::native::{Bridge, Update};
use crate::{App, Command, Wake};

/// `RegisterClassW`'s error when an earlier start registered the class.
const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A window that is never shown, for the controls to belong to.
fn hidden_window(app: &App) -> Result<HWND, String> {
    let class_name = wide(&format!("{}MediaControls", app.bus_name));
    let title = wide(&app.identity);
    // SAFETY: a null name asks for this executable's own module handle.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(DefWindowProcW),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    // SAFETY: `class` and the name it points to outlive the call.
    if unsafe { RegisterClassW(&class) } == 0 {
        // SAFETY: reads this thread's last error, set by the call above.
        let error = unsafe { GetLastError() };
        if error != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!("cannot register a window class ({error})"));
        }
    }
    // SAFETY: the class is registered and the strings outlive the call; the
    // window has no parent, menu or creation data.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        Err("cannot create a window".to_owned())
    } else {
        Ok(hwnd)
    }
}

/// Runs the controls on their own thread. Answers with the thread's id once
/// they exist, or with why they could not be made.
pub(crate) fn start(
    app: App,
    sender: Sender<Command>,
    wake: Wake,
    updates: Receiver<Update>,
) -> Result<u32, String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("fastframe-media-controls".to_owned())
        .spawn(move || {
            // The controls are WinRT objects, which want COM on the thread
            // that makes them; apartment-threaded, so their callbacks arrive
            // through the message loop below.
            // SAFETY: initialises COM for this thread only, once.
            unsafe {
                CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            }
            let mut bridge = match hidden_window(&app)
                .and_then(|hwnd| Bridge::new(&app, Some(hwnd), sender, wake))
            {
                Ok(bridge) => bridge,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            // SAFETY: no arguments; returns this thread's id.
            let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
            // SAFETY: an all-zero MSG is a valid value for GetMessageW to fill.
            let mut message: MSG = unsafe { std::mem::zeroed() };
            // SAFETY: `message` outlives each call; a null window takes every
            // message for this thread.
            while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                if message.message == WM_APP {
                    while let Ok(update) = updates.try_recv() {
                        match update {
                            Update::State(state) => bridge.apply(*state),
                            Update::Seeked(position) => bridge.seeked(position),
                        }
                    }
                    continue;
                }
                // SAFETY: `message` came from GetMessageW just now.
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        });
    if let Err(error) = spawned {
        return Err(error.to_string());
    }
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "the media controls' thread did not answer".to_owned())?
}

/// Wakes the controls' message loop to read what was sent to it.
pub(crate) fn poke(thread: u32) {
    // SAFETY: posts a message with no pointers to a thread id this process
    // started; a thread that has ended makes the call fail harmlessly.
    unsafe {
        PostThreadMessageW(thread, WM_APP, 0, 0);
    }
}
