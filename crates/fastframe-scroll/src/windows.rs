//! The Windows mouse setting for how far a wheel notch scrolls.
#![allow(unsafe_code)]

use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWHEELSCROLLLINES, SystemParametersInfoW};

/// How many lines a wheel notch scrolls (Settings, Bluetooth & devices,
/// Mouse), `u32::MAX` for a screen at a time, or `None` when it cannot be
/// read.
pub(crate) fn wheel_lines() -> Option<u32> {
    let mut lines: u32 = 0;
    // SAFETY: SPI_GETWHEELSCROLLLINES writes one UINT to the pointer, which
    // points at `lines`, alive for the call.
    let read =
        unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, (&raw mut lines).cast(), 0) };
    (read != 0).then_some(lines)
}
