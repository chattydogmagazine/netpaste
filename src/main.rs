#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! NetPaste - one app on every machine. Paste or type into the box, hit Send,
//! and it lands in the same box on every other device running the app.

use netpaste_app::{config, crypto, net, proto, spell, theme, update};

use config::Config;
use eframe::egui;
use net::{Event, Health, Incoming, Net, PeerView};
use proto::{now_secs, FileMeta};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// The accent colour of whichever skin is on. egui carries it in the style
/// so every widget can reach it without threading config through the UI.
fn accent(ui: &egui::Ui) -> egui::Color32 {
    ui.visuals().selection.bg_fill
}
const ONLINE: egui::Color32 = egui::Color32::from_rgb(56, 178, 96);
const QUIET: egui::Color32 = egui::Color32::from_rgb(220, 160, 50);
const UNREACHABLE: egui::Color32 = egui::Color32::from_rgb(214, 74, 74);

fn health_color(h: Health) -> egui::Color32 {
    match h {
        Health::Online => ONLINE,
        Health::Quiet => QUIET,
        Health::Unreachable => UNREACHABLE,
    }
}

/// Bubble colours, derived from the skin that is on rather than fixed, so
/// every theme gets readable text without its own table of values.
fn bubble_colors(ui: &egui::Ui, mine: bool) -> (egui::Color32, egui::Color32) {
    let v = ui.visuals();
    // Built from panel_fill rather than from faint_bg_color, which egui
    // stores as an additive colour with zero alpha - using it directly gives
    // nearly invisible text.
    let ink = theme::ink_for([v.panel_fill.r(), v.panel_fill.g(), v.panel_fill.b()]);
    let fill =
        if mine { mix(v.panel_fill, accent(ui), 0.26) } else { mix(v.panel_fill, ink, 0.09) };
    // Pick the text colour from the bubble it sits on, so no skin can produce
    // an unreadable pairing.
    (fill, theme::ink_for([fill.r(), fill.g(), fill.b()]))
}

/// `t` of the way from `a` to `b`.
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn sender_color(ui: &egui::Ui, mine: bool) -> egui::Color32 {
    if mine {
        accent(ui)
    } else if ui.visuals().dark_mode {
        egui::Color32::from_rgb(130, 210, 150)
    } else {
        egui::Color32::from_rgb(28, 110, 58)
    }
}

/// Secondary text. The high-contrast skin keeps it near full strength -
/// greying out small print is exactly what makes an interface unusable for
/// someone who needed the skin in the first place.
fn dim(ui: &egui::Ui) -> egui::Color32 {
    let v = ui.visuals();
    if theme::is_high_contrast(ui.ctx()) {
        let ink = v.override_text_color.unwrap_or_else(|| v.text_color());
        mix(v.panel_fill, ink, 0.85)
    } else {
        v.weak_text_color()
    }
}

/// A status dot. Painted, not typed: ● is missing from the bundled fonts.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 700.0])
            .with_min_inner_size([640.0, 440.0])
            .with_title("NetPaste"),
        ..Default::default()
    };
    eframe::run_native(
        "NetPaste",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

// ------------------------------------------------------------------ model

struct Attachment {
    meta: FileMeta,
    /// Where the file lives on the sender's machine (only set for our own).
    local: Option<PathBuf>,
}

struct Message {
    id: String,
    from_name: String,
    mine: bool,
    ts: u64,
    body: String,
    files: Vec<Attachment>,
    origin: Option<SocketAddr>,
    delivery: Option<(usize, usize)>,
}

#[derive(Clone)]
enum Transfer {
    Running { done: u64, total: u64 },
    Saved(PathBuf),
    Failed(String),
}

struct App {
    cfg: Config,
    net: Option<Arc<Net>>,
    rx: Option<Receiver<Event>>,
    start_error: Option<String>,

    peers: Vec<PeerView>,
    messages: Vec<Message>,
    transfers: HashMap<String, Transfer>,

    compose: String,
    pending: Vec<PathBuf>,
    note: Option<(String, f64)>,

    settings_open: bool,
    draft_name: String,
    draft_key: String,
    send_shortcut: bool,

    /// A newer version seen on another device: who has it and which version.
    update_offer: Option<(String, String)>,
    update_progress: Option<(u64, u64)>,
    update_error: Option<String>,

    /// Misspellings in the compose box, and the text they were found in.
    spell_bad: Vec<std::ops::Range<usize>>,
    spell_src: String,
    last_update_check: std::time::Instant,
    /// What the skin was last applied for, so it is only re-applied on change.
    applied_theme: Option<(theme::Theme, [u8; 3], bool)>,
}

/// Whether the operating system is currently in dark mode.
fn system_is_dark(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.raw.system_theme) == Some(egui::Theme::Dark)
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> App {
        let mut cfg = Config::load();
        let first_run = cfg.room_key.trim().is_empty();
        if cfg.device_id.is_empty() {
            cfg.device_id = crypto::random_hex(8);
        }

        let mut app = App {
            draft_name: cfg.device_name.clone(),
            draft_key: if first_run { crypto::suggest_room_key() } else { cfg.room_key.clone() },
            cfg,
            net: None,
            rx: None,
            start_error: None,
            peers: Vec::new(),
            messages: Vec::new(),
            transfers: HashMap::new(),
            compose: String::new(),
            pending: Vec::new(),
            note: None,
            settings_open: first_run,
            send_shortcut: false,
            update_offer: None,
            update_progress: None,
            update_error: None,
            spell_bad: Vec::new(),
            spell_src: String::new(),
            last_update_check: std::time::Instant::now(),
            applied_theme: None,
        };
        theme::apply(&cc.egui_ctx, app.cfg.theme, system_is_dark(&cc.egui_ctx), app.cfg.background);
        app.applied_theme = Some((app.cfg.theme, app.cfg.background, system_is_dark(&cc.egui_ctx)));
        update::clean_old_version();
        if !first_run {
            app.start_net(&cc.egui_ctx);
        }
        app
    }

    fn start_net(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        match Net::start(
            self.cfg.device_id.clone(),
            self.cfg.device_name.clone(),
            &self.cfg.room_key,
            self.cfg.tcp_port,
            self.cfg.disc_port,
            Box::new(move || ctx.request_repaint()),
        ) {
            Ok((net, rx)) => {
                self.net = Some(net);
                self.rx = Some(rx);
                self.start_error = None;
            }
            Err(e) => {
                self.start_error = Some(format!(
                    "Could not open the network ports ({}). Another copy of NetPaste may \
                     already be running, or the firewall is blocking it.",
                    e
                ));
            }
        }
    }

    /// Skins are applied to the whole context, so this watches for the three
    /// things that can change the answer: the choice, the chosen background,
    /// and the system flipping between light and dark.
    fn keep_theme_current(&mut self, ctx: &egui::Context) {
        let now = (self.cfg.theme, self.cfg.background, system_is_dark(ctx));
        if self.applied_theme != Some(now) {
            theme::apply(ctx, now.0, now.2, now.1);
            self.applied_theme = Some(now);
        }
    }

    fn drain_events(&mut self) {
        let Some(rx) = &self.rx else { return };
        let events: Vec<Event> = rx.try_iter().collect();
        for ev in events {
            match ev {
                Event::Peers(list) => self.peers = list,
                Event::Incoming(inc) => self.add_incoming(inc),
                Event::Delivered { msg_id, ok, failed } => {
                    if let Some(m) = self.messages.iter_mut().find(|m| m.id == msg_id) {
                        m.delivery = Some((ok, failed));
                    }
                }
                Event::Progress { file_id, done, total } => {
                    self.transfers.insert(file_id, Transfer::Running { done, total });
                }
                Event::Saved { file_id, path } => {
                    self.note(format!("Saved to {}", path.display()));
                    self.transfers.insert(file_id, Transfer::Saved(path));
                }
                Event::Failed { file_id, reason } => {
                    self.transfers.insert(file_id, Transfer::Failed(reason));
                }
                Event::UpdateAvailable { from_name, version } => {
                    self.update_offer = Some((from_name, version));
                    if self.cfg.auto_update {
                        self.take_update();
                    }
                }
                Event::UpdateProgress { done, total } => {
                    self.update_progress = Some((done, total));
                }
                Event::UpdateFailed(reason) => {
                    self.update_progress = None;
                    self.update_error = Some(reason);
                }
                Event::Note(text) => self.note(text),
            }
        }
    }

    /// Pull the newer build a device in this room is advertising. The app
    /// restarts into it once it has been checked.
    fn take_update(&mut self) {
        let Some(net) = &self.net else { return };
        let Some((name, version, sha, addr)) = net.update_source() else { return };
        update::log(format!("taking v{} from {} at {}", version, name, addr));
        self.update_error = None;
        self.update_progress = Some((0, 0));
        net.pull_update(addr, sha);
    }

    /// Updates are also checked on a timer, not only when an announcement
    /// happens to change - a device that missed the moment should not be
    /// stranded on an old version.
    fn poll_for_update(&mut self) {
        if !self.cfg.auto_update || self.update_progress.is_some() {
            return;
        }
        if self.last_update_check.elapsed() < std::time::Duration::from_secs(10) {
            return;
        }
        self.last_update_check = std::time::Instant::now();
        if self.net.as_ref().and_then(|n| n.update_source()).is_some() {
            self.take_update();
        }
    }

    fn add_incoming(&mut self, inc: Incoming) {
        if self.messages.iter().any(|m| m.id == inc.msg_id) {
            return;
        }
        self.messages.push(Message {
            id: inc.msg_id,
            from_name: inc.from_name,
            mine: false,
            ts: inc.ts,
            body: inc.body,
            files: inc.files.into_iter().map(|meta| Attachment { meta, local: None }).collect(),
            origin: Some(inc.origin),
            delivery: None,
        });
        self.trim_history();
    }

    /// Keep the history inside the limits chosen in settings. Nothing is ever
    /// written to disk, so this is what stops a long-running app from growing
    /// without bound - and it releases the files an old message was offering.
    fn trim_history(&mut self) {
        let mut dropped: Vec<String> = Vec::new();

        // Attachments first: count items, newest backwards, and cut the rest.
        let mut kept_files = 0usize;
        let mut cut_from = None;
        for (i, m) in self.messages.iter().enumerate().rev() {
            if m.files.is_empty() {
                continue;
            }
            if kept_files >= self.cfg.max_files {
                cut_from = Some(i);
                break;
            }
            kept_files += m.files.len();
        }
        if let Some(limit) = cut_from {
            for m in &mut self.messages[..=limit] {
                dropped.extend(m.files.drain(..).map(|a| a.meta.id));
            }
        }

        // Then the message count itself.
        if self.messages.len() > self.cfg.max_messages {
            let excess = self.messages.len() - self.cfg.max_messages;
            for m in self.messages.drain(..excess) {
                dropped.extend(m.files.into_iter().map(|a| a.meta.id));
            }
        }

        // A message that is now just an empty husk is not worth showing.
        self.messages.retain(|m| !(m.body.is_empty() && m.files.is_empty()));

        if !dropped.is_empty() {
            for id in &dropped {
                self.transfers.remove(id);
            }
            if let Some(net) = &self.net {
                net.unoffer(&dropped);
            }
        }
    }

    fn note(&mut self, text: String) {
        self.note = Some((text, now_secs() as f64));
    }

    fn send(&mut self) {
        let Some(net) = self.net.clone() else { return };
        let body = self.compose.trim_end().to_string();
        if body.is_empty() && self.pending.is_empty() {
            return;
        }
        if self.peers.is_empty() {
            self.note("No other devices are online yet - sent to nobody.".into());
        }

        let metas = net.register_files(&self.pending);
        let msg_id = crypto::random_hex(8);
        net.post(msg_id.clone(), body.clone(), metas.clone());

        let files = metas
            .into_iter()
            .zip(self.pending.iter().cloned())
            .map(|(meta, path)| Attachment { meta, local: Some(path) })
            .collect();

        self.messages.push(Message {
            id: msg_id,
            from_name: self.cfg.device_name.clone(),
            mine: true,
            ts: now_secs(),
            body,
            files,
            origin: None,
            delivery: None,
        });
        self.trim_history();
        self.compose.clear();
        self.pending.clear();
    }

    fn pick_files(&mut self) {
        if let Some(paths) = rfd::FileDialog::new().set_title("Attach files").pick_files() {
            self.pending.extend(paths);
        }
    }

    fn save_attachment(&mut self, origin: SocketAddr, meta: &FileMeta) {
        let mut dlg = rfd::FileDialog::new().set_file_name(&meta.name).set_title("Save file");
        if let Some(dir) = &self.cfg.save_dir {
            dlg = dlg.set_directory(dir);
        }
        let Some(dest) = dlg.save_file() else { return };
        if let Some(parent) = dest.parent() {
            self.cfg.save_dir = Some(parent.to_path_buf());
            self.cfg.save();
        }
        if let Some(net) = &self.net {
            self.transfers
                .insert(meta.id.clone(), Transfer::Running { done: 0, total: meta.size });
            net.fetch(origin, meta.clone(), dest);
        }
    }

    fn apply_settings(&mut self, ctx: &egui::Context) {
        let name = self.draft_name.trim().to_string();
        let key = self.draft_key.trim().to_string();
        if key.is_empty() {
            self.note("The room key cannot be empty.".into());
            return;
        }
        let was_empty = self.cfg.room_key.trim().is_empty();
        let key_changed = key != self.cfg.room_key;
        self.cfg.device_name = if name.is_empty() { config::default_device_name() } else { name };
        self.cfg.room_key = key;
        self.cfg.save();
        // Lowering a limit should take effect straight away, not at the next
        // message.
        self.trim_history();

        match (&self.net, was_empty) {
            (None, _) => self.start_net(ctx),
            (Some(net), _) => {
                net.set_name(&self.cfg.device_name);
                if key_changed {
                    net.set_room_key(&self.cfg.room_key);
                    self.peers.clear();
                    self.note("Room key changed - looking for devices again.".into());
                }
            }
        }
        self.settings_open = false;
    }
}

// --------------------------------------------------------------------- ui

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.poll_for_update();
        self.keep_theme_current(ctx);

        self.top_bar(ctx);
        self.device_panel(ctx);
        self.action_bar(ctx);
        self.compose_panel(ctx);
        self.history(ctx);
        self.settings_window(ctx);
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        // The room key is written the moment it is set, and again here on
        // close, so it only ever has to be typed once per machine. The guard
        // stops an app still sitting on the welcome screen from autosaving an
        // empty key over a configuration that already had one.
        if !self.cfg.room_key.trim().is_empty() {
            self.cfg.save();
        }
    }
}

impl App {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let label = "☰";
                if ui
                    .button(label)
                    .on_hover_text("Show or hide the device list")
                    .clicked()
                {
                    self.cfg.show_devices = !self.cfg.show_devices;
                    self.cfg.save();
                }
                ui.heading("NetPaste");
                let dim_c = dim(ui);
                ui.label(
                    egui::RichText::new(format!("· {}", self.cfg.device_name)).color(dim_c),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Settings").clicked() {
                        self.draft_name = self.cfg.device_name.clone();
                        self.draft_key = self.cfg.room_key.clone();
                        self.settings_open = true;
                    }
                    let online = self.peers.len();
                    let (dot, text) = if online > 0 {
                        (egui::Color32::from_rgb(70, 200, 120), format!("{} device(s) online", online))
                    } else {
                        (egui::Color32::GRAY, "no other devices yet".to_string())
                    };
                    ui.label(egui::RichText::new(text).color(dim(ui)));
                    status_dot(ui, dot);
                });
            });
            ui.add_space(4.0);
        });
    }

    fn device_panel(&mut self, ctx: &egui::Context) {
        if !self.cfg.show_devices {
            return;
        }
        egui::SidePanel::left("devices")
            .resizable(true)
            .default_width(230.0)
            .width_range(180.0..=340.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.strong("Devices");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("◀").on_hover_text("Hide this list").clicked() {
                            self.cfg.show_devices = false;
                            self.cfg.save();
                        }
                    });
                });
                ui.separator();

                // The key sits at the bottom of the panel, under the list.
                egui::TopBottomPanel::bottom("device_key")
                    .show_separator_line(true)
                    .show_inside(ui, |ui| {
                        ui.add_space(4.0);
                        egui::CollapsingHeader::new("What the colours mean")
                            .default_open(true)
                            .show(ui, |ui| {
                                for (color, label) in [
                                    (accent(ui), "This device"),
                                    (ONLINE, "Online - can receive what you send"),
                                    (QUIET, "Gone quiet - no word for a few seconds"),
                                    (UNREACHABLE, "Listed, but the last send to it failed"),
                                ] {
                                    ui.horizontal(|ui| {
                                        status_dot(ui, color);
                                        let dim_c = dim(ui);
                                        ui.label(egui::RichText::new(label).small().color(dim_c));
                                    });
                                }
                                let dim_c = dim(ui);
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(
                                        "A device that is switched off or has left the \
                                         network disappears from the list.",
                                    )
                                    .small()
                                    .color(dim_c),
                                );
                            });
                        ui.add_space(4.0);
                    });

                egui::ScrollArea::vertical().show(ui, |ui| {
                    device_row(
                        ui,
                        &self.cfg.device_name,
                        &net::platform_label(),
                        &format!("this device · v{}", update::VERSION),
                        accent(ui),
                    );
                    for p in &self.peers {
                        let detail = match p.health {
                            Health::Unreachable => {
                                format!("{} · not accepting messages", p.addr.ip())
                            }
                            Health::Quiet => format!("{} · quiet for {}s", p.addr.ip(), p.seen_ago),
                            Health::Online => {
                                let v = if p.version.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · v{}", p.version)
                                };
                                format!("{}{}", p.addr.ip(), v)
                            }
                        };
                        device_row(ui, &p.name, &p.platform, &detail, health_color(p.health));
                    }
                    if self.peers.is_empty() {
                        ui.add_space(10.0);
                        let dim_c = dim(ui);
                        ui.label(
                            egui::RichText::new(
                                "Nothing else yet.\n\nOpen NetPaste on your other \
                                 machines and give them the same room key.",
                            )
                            .color(dim_c)
                            .small(),
                        );
                    }
                });
            });
    }

    /// The row of buttons that lives under the box, messenger style.
    fn action_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                if ui
                    .button("  +  ")
                    .on_hover_text("Attach files, images, documents")
                    .clicked()
                {
                    self.pick_files();
                }
                if ui
                    .button("Copy")
                    .on_hover_text("Copy the box to this machine's clipboard")
                    .clicked()
                {
                    let text = self.compose.clone();
                    ui.output_mut(|o| o.copied_text = text);
                    self.note("Copied to clipboard.".into());
                }
                if ui.button("Clear").clicked() {
                    self.compose.clear();
                    self.pending.clear();
                }

                let ready = !self.compose.trim().is_empty() || !self.pending.is_empty();
                let right_side = 210.0;
                ui.add_space((ui.available_width() - right_side).max(0.0));
                let dim_c = dim(ui);
                ui.label(egui::RichText::new(shortcut_hint()).small().color(dim_c));
                let send = ui.add_enabled(
                    ready && self.net.is_some(),
                    egui::Button::new(
                        egui::RichText::new("Send").strong().color(egui::Color32::WHITE),
                    )
                    .fill(accent(ui))
                    .min_size(egui::vec2(104.0, 30.0)),
                );
                if send.clicked() || (ready && self.send_shortcut) {
                    self.send();
                }
            });
            ui.add_space(6.0);
        });
    }

    fn compose_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("compose").show(ctx, |ui| {
            ui.add_space(6.0);

            if !self.pending.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    let mut drop = None;
                    for (i, p) in self.pending.iter().enumerate() {
                        let name = p
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                        egui::Frame::none()
                            .fill(ui.visuals().faint_bg_color)
                            .rounding(6.0)
                            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}  ({})", name, human_size(size)));
                                    if ui.small_button("×").on_hover_text("Remove").clicked() {
                                        drop = Some(i);
                                    }
                                });
                            });
                    }
                    if let Some(i) = drop {
                        self.pending.remove(i);
                    }
                });
            }

            self.send_shortcut = ui.input(|i| {
                i.key_pressed(egui::Key::Enter) && (i.modifiers.command || i.modifiers.ctrl)
            });

            self.refresh_spelling();
            let bad = self.spell_bad.clone();
            let underline = UNREACHABLE;
            let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
                let mut job = egui::text::LayoutJob::default();
                let font = egui::TextStyle::Body.resolve(ui.style());
                let color = ui.visuals().text_color();
                let plain = egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
                let flagged = egui::TextFormat {
                    font_id: font,
                    color,
                    underline: egui::Stroke::new(1.5_f32, underline),
                    ..Default::default()
                };
                let mut at = 0usize;
                for r in bad.iter().filter(|r| r.end <= text.len()) {
                    if r.start > at {
                        job.append(&text[at..r.start], 0.0, plain.clone());
                    }
                    job.append(&text[r.start..r.end], 0.0, flagged.clone());
                    at = r.end;
                }
                if at < text.len() {
                    job.append(&text[at..], 0.0, plain);
                }
                job.wrap.max_width = wrap_width;
                ui.fonts(|f| f.layout_job(job))
            };

            let output = egui::TextEdit::multiline(&mut self.compose)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Paste or type anything here, then press Send…")
                .layouter(&mut layouter)
                .show(ui);

            let caret = output
                .cursor_range
                .map(|c| c.primary.ccursor.index)
                .and_then(|idx| self.compose.char_indices().nth(idx).map(|(b, _)| b))
                .unwrap_or(self.compose.len());
            self.spelling_bar(ui, caret);
            ui.add_space(2.0);
        });
    }

    /// Re-check the box only when its contents have actually changed.
    fn refresh_spelling(&mut self) {
        if self.spell_src == self.compose {
            return;
        }
        self.spell_src = self.compose.clone();
        // Checking a whole pasted document word by word is not worth it; this
        // is a paste box, and long pastes are usually logs or keys anyway.
        self.spell_bad = if self.compose.len() <= 20_000 {
            spell::check(&self.compose)
        } else {
            Vec::new()
        };
    }

    /// Corrections for the word the caret is sitting in.
    fn spelling_bar(&mut self, ui: &mut egui::Ui, caret: usize) {
        let Some(range) = self
            .spell_bad
            .iter()
            .find(|r| caret >= r.start && caret <= r.end)
            .cloned()
        else {
            return;
        };
        let word = self.compose[range.clone()].to_string();
        let options = spell::suggestions(&word);

        ui.horizontal_wrapped(|ui| {
            let dim_c = dim(ui);
            ui.label(egui::RichText::new(format!("“{}”", word)).small().color(dim_c));
            let mut replacement = None;
            for s in options.iter().take(5) {
                if ui.small_button(s).clicked() {
                    replacement = Some(s.clone());
                }
            }
            if options.is_empty() {
                ui.label(egui::RichText::new("no suggestions").small().color(dim_c));
            }
            if ui.small_button("Learn").on_hover_text("Add to this Mac's dictionary").clicked() {
                spell::learn(&word);
                self.spell_src.clear(); // force a re-check
            }
            if let Some(s) = replacement {
                self.compose.replace_range(range.clone(), &s);
                self.spell_src.clear();
            }
        });
    }

    /// Tells you when another device has a newer build, and what is happening
    /// about it.
    fn update_banner(&mut self, ui: &mut egui::Ui) {
        if let Some((done, total)) = self.update_progress {
            egui::Frame::none()
                .fill(ui.visuals().faint_bg_color)
                .rounding(8.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Updating NetPaste…");
                        let f = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        ui.add(egui::ProgressBar::new(f).desired_width(160.0).show_percentage());
                        ui.label(
                            egui::RichText::new("the app will restart when it is ready").small(),
                        );
                    });
                });
            return;
        }
        if let Some(err) = self.update_error.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(UNREACHABLE, format!("Update failed: {}", err));
                if ui.small_button("Dismiss").clicked() {
                    self.update_error = None;
                }
            });
        }
        let still_newer = self.net.as_ref().and_then(|n| n.update_source()).is_some();
        if !still_newer {
            self.update_offer = None;
            return;
        }
        if let Some((who, version)) = self.update_offer.clone() {
            if !self.cfg.auto_update {
                egui::Frame::none()
                    .fill(ui.visuals().faint_bg_color)
                    .rounding(8.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} is running v{} - this device has v{}.",
                                who,
                                version,
                                update::VERSION
                            ));
                            if ui
                                .add(egui::Button::new("Update now").fill(accent(ui)))
                                .clicked()
                            {
                                self.take_update();
                            }
                            if ui.small_button("Not now").clicked() {
                                self.update_offer = None;
                            }
                        });
                    });
            }
        }
    }

    fn history(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(err) = &self.start_error {
                ui.add_space(20.0);
                ui.colored_label(egui::Color32::from_rgb(230, 110, 110), err);
                return;
            }
            if let Some((text, at)) = self.note.clone() {
                if now_secs() as f64 - at < 6.0 {
                    ui.colored_label(egui::Color32::from_rgb(200, 180, 90), text);
                } else {
                    self.note = None;
                }
            }
            self.update_banner(ui);

            if self.messages.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    let dim_c = dim(ui);
                    ui.label(
                        egui::RichText::new("Nothing shared yet").size(18.0).color(dim_c),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "Paste text into the box below and press Send.\n\
                             It appears here, and in the same app on every other device.",
                        )
                        .color(dim_c),
                    );
                });
                return;
            }

            let mut save_request: Option<(SocketAddr, FileMeta)> = None;
            let mut copy_request: Option<String> = None;
            let mut reuse_request: Option<String> = None;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    let width = ui.available_width();
                    let bubble = (width * 0.78).max(240.0);
                    for m in &self.messages {
                        let align = if m.mine { egui::Align::RIGHT } else { egui::Align::LEFT };
                        ui.with_layout(egui::Layout::top_down(align), |ui| {
                            let (fill, text_col) = bubble_colors(ui, m.mine);
                            egui::Frame::none()
                                .fill(fill)
                                .rounding(10.0)
                                .inner_margin(10.0)
                                .show(ui, |ui| {
                                    ui.set_max_width(bubble);
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(if m.mine {
                                                    "You".to_string()
                                                } else {
                                                    m.from_name.clone()
                                                })
                                                .strong()
                                                .color(sender_color(ui, m.mine)),
                                            );
                                            ui.label(
                                                egui::RichText::new(ago(m.ts)).small().color(dim(ui)),
                                            );
                                            if let Some((ok, failed)) = m.delivery {
                                                let t = if failed == 0 {
                                                    format!("sent to {}", ok)
                                                } else {
                                                    format!("sent to {} · {} failed", ok, failed)
                                                };
                                                ui.label(egui::RichText::new(t).small().color(dim(ui)));
                                            }
                                        });

                                        if !m.body.is_empty() {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(m.body.clone())
                                                        .color(text_col),
                                                )
                                                .wrap(),
                                            );
                                        }

                                        for att in &m.files {
                                            ui.add_space(4.0);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{}  ({})",
                                                        att.meta.name,
                                                        human_size(att.meta.size)
                                                    ))
                                                    .color(text_col)
                                                    .italics(),
                                                );
                                                match self.transfers.get(&att.meta.id) {
                                                    Some(Transfer::Running { done, total }) => {
                                                        let f = if *total > 0 {
                                                            *done as f32 / *total as f32
                                                        } else {
                                                            0.0
                                                        };
                                                        ui.add(
                                                            egui::ProgressBar::new(f)
                                                                .desired_width(110.0)
                                                                .show_percentage(),
                                                        );
                                                    }
                                                    Some(Transfer::Saved(p)) => {
                                                        ui.colored_label(
                                                            egui::Color32::LIGHT_GREEN,
                                                            "saved",
                                                        )
                                                        .on_hover_text(p.display().to_string());
                                                    }
                                                    Some(Transfer::Failed(e)) => {
                                                        ui.colored_label(
                                                            egui::Color32::from_rgb(230, 110, 110),
                                                            "failed",
                                                        )
                                                        .on_hover_text(e.clone());
                                                    }
                                                    None => {}
                                                }
                                                if let Some(origin) = m.origin {
                                                    if ui.small_button("Save…").clicked() {
                                                        save_request =
                                                            Some((origin, att.meta.clone()));
                                                    }
                                                } else if att.local.is_some() {
                                                    ui.label(
                                                        egui::RichText::new("shared")
                                                            .small()
                                                            .color(dim(ui)),
                                                    );
                                                }
                                            });
                                        }

                                        if !m.body.is_empty() {
                                            ui.add_space(2.0);
                                            ui.horizontal(|ui| {
                                                if ui.small_button("Copy").clicked() {
                                                    copy_request = Some(m.body.clone());
                                                }
                                                if ui.small_button("Edit & resend").clicked() {
                                                    reuse_request = Some(m.body.clone());
                                                }
                                            });
                                        }
                                    });
                                });
                        });
                        ui.add_space(8.0);
                    }
                });

            if let Some((origin, meta)) = save_request {
                self.save_attachment(origin, &meta);
            }
            if let Some(text) = copy_request {
                ui.output_mut(|o| o.copied_text = text);
                self.note("Copied to clipboard.".into());
            }
            if let Some(text) = reuse_request {
                self.compose = text;
            }
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let first_run = self.cfg.room_key.trim().is_empty();
        let mut open = !first_run;
        let mut apply = false;

        let mut window = egui::Window::new(if first_run { "Welcome to NetPaste" } else { "Settings" })
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(460.0);
        if !first_run {
            window = window.open(&mut open);
        }

        window.show(ctx, |ui| {
            if first_run {
                ui.label(
                    "NetPaste shares text and files between your own machines over the local \
                     network. Give every device the same room key - devices with different \
                     keys cannot see each other, and everything sent is encrypted with it.",
                );
                ui.add_space(8.0);
            }

            ui.label("This device is called");
            ui.add(egui::TextEdit::singleline(&mut self.draft_name).desired_width(f32::INFINITY));
            ui.add_space(10.0);

            ui.label("Room key (type the same one on every device)");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft_key)
                        .desired_width(ui.available_width() - 150.0),
                );
                if ui.button("Copy").clicked() {
                    let k = self.draft_key.clone();
                    ui.output_mut(|o| o.copied_text = k);
                }
                if ui.button("New key").clicked() {
                    self.draft_key = crypto::suggest_room_key();
                }
            });

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);
            ui.strong("Appearance");
            ui.horizontal_wrapped(|ui| {
                for t in theme::Theme::ALL {
                    if ui
                        .selectable_label(self.cfg.theme == t, t.name())
                        .on_hover_text(t.blurb())
                        .clicked()
                    {
                        self.cfg.theme = t;
                        self.cfg.save();
                    }
                }
            });
            if self.cfg.theme == theme::Theme::Accessible {
                ui.horizontal(|ui| {
                    ui.label("Background");
                    if ui.color_edit_button_srgb(&mut self.cfg.background).changed() {
                        self.cfg.save();
                    }
                    for (label, rgb) in [
                        ("Black", [0u8, 0, 0]),
                        ("Cream", [250, 243, 224]),
                        ("Navy", [12, 22, 48]),
                        ("Yellow", [255, 234, 120]),
                    ] {
                        if ui.small_button(label).clicked() {
                            self.cfg.background = rgb;
                            self.cfg.save();
                        }
                    }
                });
                let dim_c = dim(ui);
                ui.label(
                    egui::RichText::new(
                        "Text and borders switch between black and white automatically, \
                         whichever stays readable on the colour you pick.",
                    )
                    .small()
                    .color(dim_c),
                );
            }

            ui.add_space(12.0);
            ui.strong("History");
            let dim_c = dim(ui);
            ui.label(
                egui::RichText::new(
                    "Kept in memory only, never written to disk. Older items are discarded.",
                )
                .small()
                .color(dim_c),
            );
            ui.horizontal(|ui| {
                ui.label("Keep the last");
                egui::ComboBox::from_id_salt("max_messages")
                    .selected_text(format!("{}", self.cfg.max_messages))
                    .show_ui(ui, |ui| {
                        for n in config::MESSAGE_LIMITS {
                            ui.selectable_value(&mut self.cfg.max_messages, n, n.to_string());
                        }
                    });
                ui.label("messages, and");
                egui::ComboBox::from_id_salt("max_files")
                    .selected_text(format!("{}", self.cfg.max_files))
                    .show_ui(ui, |ui| {
                        for n in config::FILE_LIMITS {
                            ui.selectable_value(&mut self.cfg.max_files, n, n.to_string());
                        }
                    });
                ui.label("attachments");
            });

            ui.add_space(10.0);
            ui.strong("Updates");
            ui.checkbox(
                &mut self.cfg.auto_update,
                "Take new versions from my other devices automatically",
            );
            let dim_c = dim(ui);
            ui.label(
                egui::RichText::new(format!(
                    "This device runs v{}. When one machine is updated, the others in \
                     the room pick the new version up from it and restart. Anyone with \
                     the room key can hand your devices a new build, so keep the key to \
                     machines you trust.",
                    update::VERSION
                ))
                .small()
                .color(dim_c),
            );

            ui.add_space(10.0);
            ui.collapsing("Details", |ui| {
                ui.label(format!("Device id: {}", self.cfg.device_id));
                if let Some(net) = &self.net {
                    ui.label(format!("Listening on TCP {}", net.tcp_port));
                    ui.label(format!("Discovery on UDP {}", net.disc_port));
                }
                if let Some(ip) = net::local_ip() {
                    ui.label(format!("This machine: {}", ip));
                }
                ui.label(format!("Settings file: {}", config::config_path().display()));
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(if first_run {
                            "Start"
                        } else {
                            "Apply"
                        }))
                        .fill(accent(ui)),
                    )
                    .clicked()
                {
                    apply = true;
                }
                if !first_run && ui.button("Cancel").clicked() {
                    self.settings_open = false;
                }
            });
        });

        if apply {
            self.apply_settings(ctx);
        } else if !first_run && !open {
            self.settings_open = false;
        }
    }
}

// ----------------------------------------------------------------- helpers

fn device_row(ui: &mut egui::Ui, name: &str, platform: &str, detail: &str, dot: egui::Color32) {
    egui::Frame::none()
        .inner_margin(egui::Margin::symmetric(4.0, 6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                status_dot(ui, dot);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(name).strong());
                    let dim_c = dim(ui);
                    ui.label(egui::RichText::new(platform).small().color(dim_c));
                    ui.label(egui::RichText::new(detail).small().color(dim_c));
                });
            });
        });
    ui.separator();
}

fn shortcut_hint() -> &'static str {
    if cfg!(target_os = "macos") { "⌘ + Enter" } else { "Ctrl + Enter" }
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.1} {}", v, UNITS[i])
    }
}

fn ago(ts: u64) -> String {
    let d = now_secs().saturating_sub(ts);
    match d {
        0..=5 => "just now".into(),
        6..=59 => format!("{}s ago", d),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86400),
    }
}
