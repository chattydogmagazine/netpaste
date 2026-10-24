//! Skins.
//!
//! Each one is a complete look: colours, corner rounding and text sizing.
//! They exist because the same app gets used in very different places - a
//! bright office, a dark studio, a machine whose owner needs far more
//! contrast than the default gives them.

use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    /// Follows whatever light/dark setting the operating system is in.
    #[default]
    System,
    /// Soft, rounded and colourful.
    Playful,
    /// Low light, for working at night.
    Dark,
    /// Grey panels and square corners, like a desktop app from the nineties.
    OldSchool,
    /// Maximum contrast, larger text, and a background colour you choose.
    Accessible,
}

impl Theme {
    pub const ALL: [Theme; 5] =
        [Theme::System, Theme::Playful, Theme::Dark, Theme::OldSchool, Theme::Accessible];

    pub fn name(self) -> &'static str {
        match self {
            Theme::System => "System",
            Theme::Playful => "Playful",
            Theme::Dark => "Dark",
            Theme::OldSchool => "Old school",
            Theme::Accessible => "High contrast",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Theme::System => "Follows your computer's light or dark setting.",
            Theme::Playful => "Soft, rounded and warm.",
            Theme::Dark => "Easy on the eyes at night.",
            Theme::OldSchool => "Square corners and grey panels.",
            Theme::Accessible => "Strong contrast, bigger text, your choice of background.",
        }
    }

    /// Accent colour for buttons and the current device's dot.
    pub fn accent(self, dark: bool) -> egui::Color32 {
        match self {
            Theme::Playful => egui::Color32::from_rgb(235, 110, 90),
            Theme::OldSchool => egui::Color32::from_rgb(0, 60, 140),
            Theme::Accessible => {
                if dark {
                    egui::Color32::from_rgb(255, 214, 0)
                } else {
                    egui::Color32::from_rgb(0, 60, 190)
                }
            }
            _ => egui::Color32::from_rgb(52, 120, 220),
        }
    }
}

/// True when the resulting look is a dark one, which the rest of the UI needs
/// in order to pick readable text colours.
pub fn is_dark(theme: Theme, system_dark: bool, background: [u8; 3]) -> bool {
    match theme {
        Theme::System => system_dark,
        Theme::Dark => true,
        Theme::Playful | Theme::OldSchool => false,
        Theme::Accessible => luminance(background) < 0.5,
    }
}

/// Perceived brightness, 0.0 (black) to 1.0 (white).
pub fn luminance(c: [u8; 3]) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

/// Black or white, whichever is readable on the given background.
pub fn ink_for(bg: [u8; 3]) -> egui::Color32 {
    if luminance(bg) < 0.5 {
        egui::Color32::from_rgb(250, 250, 250)
    } else {
        egui::Color32::from_rgb(10, 10, 10)
    }
}

/// Set by `apply` so the rest of the UI can ask whether it must keep every
/// piece of text at full contrast, including the secondary labels that would
/// normally be greyed out.
pub fn high_contrast_id() -> egui::Id {
    egui::Id::new("netpaste-high-contrast")
}

pub fn is_high_contrast(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(high_contrast_id()).unwrap_or(false))
}

/// Apply a skin to the whole app.
pub fn apply(ctx: &egui::Context, theme: Theme, system_dark: bool, background: [u8; 3]) {
    let dark = is_dark(theme, system_dark, background);
    let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let mut style = (*ctx.style()).clone();

    // Start from sensible defaults, then let each skin diverge.
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    for (_, font) in style.text_styles.iter_mut() {
        font.size = default_size(font.size);
    }

    match theme {
        Theme::System => {}

        Theme::Playful => {
            visuals.panel_fill = egui::Color32::from_rgb(255, 250, 244);
            visuals.window_fill = egui::Color32::from_rgb(255, 250, 244);
            visuals.extreme_bg_color = egui::Color32::from_rgb(255, 255, 255);
            visuals.faint_bg_color = egui::Color32::from_rgb(255, 238, 226);
            visuals.override_text_color = Some(egui::Color32::from_rgb(58, 42, 38));
            round_everything(&mut visuals, 12.0);
            style.spacing.button_padding = egui::vec2(12.0, 8.0);
        }

        Theme::Dark => {
            visuals.panel_fill = egui::Color32::from_rgb(22, 24, 29);
            visuals.window_fill = egui::Color32::from_rgb(26, 28, 34);
            visuals.extreme_bg_color = egui::Color32::from_rgb(16, 17, 21);
            visuals.faint_bg_color = egui::Color32::from_rgb(38, 41, 48);
            visuals.override_text_color = Some(egui::Color32::from_rgb(225, 228, 234));
            round_everything(&mut visuals, 8.0);
        }

        Theme::OldSchool => {
            let face = egui::Color32::from_rgb(212, 208, 200); // the classic grey
            visuals.panel_fill = face;
            visuals.window_fill = face;
            visuals.extreme_bg_color = egui::Color32::from_rgb(255, 255, 255);
            visuals.faint_bg_color = egui::Color32::from_rgb(198, 194, 186);
            visuals.override_text_color = Some(egui::Color32::BLACK);
            round_everything(&mut visuals, 0.0);
            visuals.widgets.inactive.bg_fill = face;
            visuals.widgets.inactive.weak_bg_fill = face;
            visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(222, 219, 212);
            // Hard light/dark edges instead of soft modern outlines.
            visuals.widgets.inactive.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(128, 128, 128));
            visuals.widgets.noninteractive.bg_stroke =
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(128, 128, 128));
            style.spacing.item_spacing = egui::vec2(6.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
        }

        Theme::Accessible => {
            let bg = egui::Color32::from_rgb(background[0], background[1], background[2]);
            let ink = ink_for(background);
            visuals.panel_fill = bg;
            visuals.window_fill = bg;
            visuals.extreme_bg_color = shift(bg, if dark { 14 } else { -10 });
            visuals.faint_bg_color = shift(bg, if dark { 26 } else { -22 });
            visuals.override_text_color = Some(ink);
            round_everything(&mut visuals, 4.0);
            // Every control gets a visible, high-contrast border.
            let edge = egui::Stroke::new(2.0_f32, ink);
            visuals.widgets.noninteractive.bg_stroke = edge;
            visuals.widgets.inactive.bg_stroke = edge;
            visuals.widgets.hovered.bg_stroke = egui::Stroke::new(3.0_f32, ink);
            visuals.widgets.active.bg_stroke = egui::Stroke::new(3.0_f32, ink);
            visuals.widgets.inactive.bg_fill = shift(bg, if dark { 20 } else { -16 });
            visuals.widgets.inactive.weak_bg_fill = shift(bg, if dark { 20 } else { -16 });
            // Bigger text, and more room around everything.
            for (_, font) in style.text_styles.iter_mut() {
                font.size = (default_size(font.size) * 1.25).round();
            }
            style.spacing.item_spacing = egui::vec2(10.0, 10.0);
            style.spacing.button_padding = egui::vec2(14.0, 9.0);
        }
    }

    // The accent lives in selection.bg_fill so any widget can reach it from
    // the style alone.
    visuals.selection.bg_fill = theme.accent(dark);
    visuals.selection.stroke.color = if dark {
        egui::Color32::from_rgb(235, 240, 250)
    } else {
        egui::Color32::from_rgb(20, 24, 30)
    };
    ctx.data_mut(|d| d.insert_temp(high_contrast_id(), theme == Theme::Accessible));
    style.visuals = visuals;
    // egui re-picks the style for the current light/dark preference every
    // frame, so pin the preference and register the style under both - a
    // style set only for the "other" theme is silently discarded.
    ctx.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
    let style = std::sync::Arc::new(style);
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

/// The text sizes every theme starts from, keyed off egui's own defaults.
fn default_size(current: f32) -> f32 {
    match current.round() as i32 {
        0..=11 => 11.0,  // small
        12..=13 => 13.0, // body / buttons
        14..=17 => 14.0, // monospace
        _ => 19.0,       // heading
    }
}

fn round_everything(v: &mut egui::Visuals, r: f32) {
    let rounding = egui::Rounding::same(r);
    v.widgets.noninteractive.rounding = rounding;
    v.widgets.inactive.rounding = rounding;
    v.widgets.hovered.rounding = rounding;
    v.widgets.active.rounding = rounding;
    v.widgets.open.rounding = rounding;
    v.window_rounding = egui::Rounding::same(r.max(4.0));
    v.menu_rounding = rounding;
}

/// Nudge a colour lighter (positive) or darker (negative).
fn shift(c: egui::Color32, by: i16) -> egui::Color32 {
    let f = |v: u8| (v as i16 + by).clamp(0, 255) as u8;
    egui::Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}
