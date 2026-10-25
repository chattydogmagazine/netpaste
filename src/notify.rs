//! Letting someone know something arrived while they were looking elsewhere.
//!
//! The cross-platform half - bouncing the Dock icon, flashing the taskbar
//! button, and the count in the window title - is handled by egui. This is
//! the part that is not: the red badge on the macOS Dock icon, which is the
//! bit that actually says *how many*.

/// Put a count on the app's Dock icon, or clear it with `None`.
/// A no-op anywhere there is no such thing.
pub fn set_badge(count: Option<usize>) {
    #[cfg(target_os = "macos")]
    mac::set_badge(count);
    #[cfg(not(target_os = "macos"))]
    let _ = count;
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;

    // NSApplication lives in AppKit, which the spell checker already links.
    pub fn set_badge(count: Option<usize>) {
        let Some(class) = AnyClass::get(c"NSApplication") else { return };
        unsafe {
            let app: *mut AnyObject = msg_send![class, sharedApplication];
            if app.is_null() {
                return;
            }
            let tile: *mut AnyObject = msg_send![app, dockTile];
            if tile.is_null() {
                return;
            }
            match count {
                // Past a point the exact number stops being useful and the
                // badge stops fitting.
                Some(n) if n > 0 => {
                    let text = if n > 99 { "99+".to_string() } else { n.to_string() };
                    let label: Retained<NSString> = NSString::from_str(&text);
                    let _: () = msg_send![tile, setBadgeLabel: &*label];
                }
                _ => {
                    let _: () = msg_send![tile, setBadgeLabel: std::ptr::null::<NSString>()];
                }
            }
        }
    }
}
