//! eframe substitutes its own egui logo when no icon is set, and on macOS
//! that becomes the Dock icon of the running app - overriding the one in the
//! bundle, so Finder shows the right icon and the Dock shows egui's. The app
//! therefore has to hand eframe a real icon at startup. If the file ever
//! moves or stops decoding, that silently reverts, and only someone looking
//! at their Dock would notice. Hence this test.

#[test]
fn the_app_icon_is_embedded_and_usable() {
    let bytes = include_bytes!("../branding/icons/icon-256.png");
    let icon = eframe::icon_data::from_png_bytes(bytes)
        .expect("the embedded app icon must decode, or eframe falls back to its own");

    assert_eq!(icon.width, 256, "icon should be 256x256");
    assert_eq!(icon.height, 256);
    assert_eq!(icon.rgba.len(), 256 * 256 * 4, "should be RGBA");

    // A fully transparent or fully blank image would "work" and look broken.
    let opaque = icon.rgba.chunks(4).filter(|p| p[3] > 128).count();
    assert!(
        opaque > 256 * 256 / 4,
        "icon looks mostly empty: only {opaque} solid pixels"
    );
}
