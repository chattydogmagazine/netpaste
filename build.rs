//! Embeds the app icon into the Windows executable, so NetPaste has its mark
//! in Explorer, the taskbar and the Start Menu rather than the blank default.
//! Everything else is a no-op.

fn main() {
    println!("cargo:rerun-if-changed=branding/icons/NetPaste.ico");

    // CARGO_CFG_TARGET_OS, not cfg!(), which would describe the machine doing
    // the building rather than the machine being built for.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("branding/icons/NetPaste.ico");
        res.set("ProductName", "NetPaste");
        res.set("FileDescription", "Shared clipboard across your own machines");
        res.set("CompanyName", "NetPaste");
        if let Err(e) = res.compile() {
            // A missing resource compiler should not stop the app being
            // built - it only costs the icon.
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
