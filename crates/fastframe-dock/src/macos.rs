//! The macOS badge: the unread count on the Dock tile, as WhatsApp shows it,
//! the unbundled Dock icon, and the activation policy that puts the app in
//! the Dock or out of it.
//!
//! The Dock tile belongs to the running process, so the label is set through
//! `NSApp.dockTile` on the main thread, where the interface runs, and an empty
//! label clears it. Setting it needs no notification permission, but macOS
//! shows it for an app registered with Notification Center only while badges
//! are allowed, which fastframe-notifications asks for.

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_foundation::NSString;

pub(super) fn set_icon(encoded_image: &[u8]) -> Result<bool, crate::IconError> {
    use objc2::AnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::{NSBundle, NSData};

    let main = MainThreadMarker::new().ok_or(crate::IconError::NotMainThread)?;
    let bundle_path = NSBundle::mainBundle().bundlePath().to_string();
    if crate::is_app_bundle(std::path::Path::new(&bundle_path)) {
        return Ok(false);
    }
    let data = NSData::with_bytes(encoded_image);
    let image =
        NSImage::initWithData(NSImage::alloc(), &data).ok_or(crate::IconError::InvalidImage)?;
    // SAFETY: main-thread marker proves AppKit affinity; the application
    // retains the decoded image after this call returns.
    unsafe { NSApplication::sharedApplication(main).setApplicationIconImage(Some(&image)) };
    Ok(true)
}

pub(super) fn set_dock_icon_visible(visible: bool) -> bool {
    use crate::Policy;
    use objc2_app_kit::NSApplicationActivationPolicy as Native;

    let Some(main) = MainThreadMarker::new() else {
        log::warn!("the Dock icon can only be shown or hidden on the main thread");
        return false;
    };
    let app = NSApplication::sharedApplication(main);
    let current = match app.activationPolicy() {
        Native::Regular => Policy::Regular,
        Native::Accessory => Policy::Accessory,
        _ => Policy::Other,
    };
    let Some(switch) = crate::dock_switch(visible, current, app.isActive()) else {
        return false;
    };
    let policy = match switch.policy {
        Policy::Accessory => Native::Accessory,
        Policy::Regular | Policy::Other => Native::Regular,
    };
    if !app.setActivationPolicy(policy) {
        log::warn!(
            "AppKit refused to {} the Dock icon",
            if visible { "show" } else { "hide" }
        );
        return false;
    }
    if switch.reactivate {
        // Leaving the Dock deactivates the app, which sends its window behind
        // the others; returning to it leaves the menu bar with the previous
        // app. Activating again keeps the window in front with the focus.
        #[allow(deprecated, reason = "its replacement needs macOS 14")]
        app.activateIgnoringOtherApps(true);
    }
    true
}

/// Counts unread chats on the Dock tile.
#[derive(Default)]
pub(super) struct Badge {
    shown: Option<u32>,
}

impl Badge {
    pub(super) fn new(_desktop_entry: String) -> Self {
        Self::default()
    }
    pub(super) fn count(&self) -> Option<u32> {
        self.shown
    }

    /// Shows the count on the Dock tile, clearing it at zero. A repeat is not
    /// applied, so calling this every frame costs nothing.
    pub(super) fn set(&mut self, count: u32) {
        if self.shown == Some(count) {
            return;
        }
        // AppKit is main-thread only; the interface always calls from there.
        let Some(main) = MainThreadMarker::new() else {
            return;
        };
        let tile = NSApplication::sharedApplication(main).dockTile();
        let label = label(count).map(|label| NSString::from_str(&label));
        tile.setBadgeLabel(label.as_deref());
        self.shown = Some(count);
    }
}

/// The Dock label for a count: none at zero, else the number.
fn label(count: u32) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_clears_the_label_and_other_counts_show_as_numbers() {
        assert_eq!(label(0), None);
        assert_eq!(label(1).as_deref(), Some("1"));
        assert_eq!(label(1234).as_deref(), Some("1234"));
    }
}
