//! Window, session list, block canvas, block library and settings panel.
use crate::audio;
use crate::dsp::{lin_to_db, EQ_FREQS};
use crate::engine::{self, Engine, Meter};
use crate::model::*;
mod extras;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

const UNIT: f32 = 34.0;
const BW: f32 = 108.0;
const BH: f32 = 92.0;

// KalangoOS colour tokens, dark mode (SDRustOS design/tokens/tokens-dark.css); the name of each token is beside it.
/// canvas and windows (cor/bg)
const BACK1: Color32 = Color32::from_rgb(0x16, 0x17, 0x1B);
/// top bars and wells (cor/panel)
const BACK2: Color32 = Color32::from_rgb(0x10, 0x11, 0x14);
/// controls and block bodies (cor/control)
const BACK3: Color32 = Color32::from_rgb(0x24, 0x25, 0x2D);
/// side panels (cor/base)
const SIDE: Color32 = Color32::from_rgb(0x12, 0x13, 0x17);
/// raised / hovered controls (cor/raised)
const RAISED: Color32 = Color32::from_rgb(0x2A, 0x2B, 0x35);
/// hairlines and grid dots (cor/divider)
const DIVIDER: Color32 = Color32::from_rgb(0x2E, 0x2F, 0x3A);
/// outlines (cor/border)
const BORDER: Color32 = Color32::from_rgb(0x26, 0x27, 0x2F);
/// text fields and meter wells (cor/field)
const FIELD: Color32 = Color32::from_rgb(0x1A, 0x1B, 0x21);
/// main text (cor/text)
const FORE1: Color32 = Color32::from_rgb(0xE2, 0xE2, 0xE6);
/// secondary text (cor/muted)
const FORE2: Color32 = Color32::from_rgb(0x8E, 0x8F, 0x99);
/// switched-off things (cor/dim)
const DIM: Color32 = Color32::from_rgb(0x7A, 0x7B, 0x88);
/// accent (cor/accent) and the text that sits on it (cor/accent_fg)
const THEME: Color32 = Color32::from_rgb(0xA5, 0xA1, 0xE2);
const ON_THEME: Color32 = Color32::from_rgb(0x1C, 0x1D, 0x24);
/// selected rows: the dark accent tint (cor/pasta_emblema)
const THEME_DARK: Color32 = Color32::from_rgb(0x2E, 0x2C, 0x52);
/// running / recording and errors (cor/red)
const LIVE: Color32 = Color32::from_rgb(0xE0, 0x6C, 0x75);
const ERR: Color32 = LIVE;
/// ready / healthy signal (cor/teal)
const GREEN: Color32 = Color32::from_rgb(0x8E, 0xD2, 0xC3);
/// warnings (cor/gold)
const WARN: Color32 = Color32::from_rgb(0xE5, 0xC9, 0x8C);
/// peaks on the level meters (cor/blue)
const PEAK: Color32 = Color32::from_rgb(0x7C, 0xC3, 0xDF);

/// What a meter shows. Bar values are 0..1 on a dB scale (-60 dBFS .. 0 dBFS), updated in ticks of 1/60 s.
/// Ballistics (our own, in the spirit of IEC 60268-10 peak meters):
/// - peak and RMS bars rise instantly and fall at 20 dB/s;
/// - the peak-hold marker jumps to every new maximum, stays for 1.5 s, then falls at 20 dB/s;
/// - the clip light turns on when a sample reaches 0 dBFS and stays on for 2 s after the last one;
/// - `vu` is the RMS level integrated like a VU meter (99 % of a step in 300 ms), same 0..1 scale.
#[derive(Clone, Copy, Default)]
pub struct Disp { peak: [f32; 2], rms: [f32; 2], hold: [f32; 2], hold_ticks: [u32; 2], clip: [f32; 2], clip_ticks: [u32; 2], vu: [f32; 2], vu_lin: [f32; 2] }

const METER_RANGE_DB: f32 = 60.0;
const METER_TICK_S: f32 = 1.0 / 60.0;
const METER_FALL_DB_PER_S: f32 = 20.0;
const METER_HOLD_S: f32 = 1.5;
const METER_CLIP_HOLD_S: f32 = 2.0;
/// One-pole time constant that reaches 99 % of a step in 300 ms: 0.3 / ln(100).
const METER_VU_TAU_S: f32 = 0.3 / 4.605_170_2;

fn norm_db(x: f32) -> f32 { ((lin_to_db(x) + METER_RANGE_DB) / METER_RANGE_DB).clamp(0.0, 1.0) }

impl Disp {
    fn update(&mut self, m: &Meter, ticks: u32) {
        let fall = METER_FALL_DB_PER_S * METER_TICK_S / METER_RANGE_DB;
        let hold_ticks = (METER_HOLD_S / METER_TICK_S).round() as u32;
        let clip_ticks = (METER_CLIP_HOLD_S / METER_TICK_S).round() as u32;
        let vu_k = 1.0 - (-METER_TICK_S / METER_VU_TAU_S).exp();
        for _ in 0..ticks.max(1) {
            for c in 0..2 {
                self.peak[c] = (self.peak[c] - fall).max(norm_db(m.peak[c])).max(0.0);
                self.rms[c] = (self.rms[c] - fall).max(norm_db(m.rms[c])).max(0.0);
                if self.peak[c] >= self.hold[c] {
                    self.hold[c] = self.peak[c];
                    self.hold_ticks[c] = hold_ticks;
                } else if self.hold_ticks[c] > 0 {
                    self.hold_ticks[c] -= 1;
                } else {
                    self.hold[c] = (self.hold[c] - fall).max(self.peak[c]).max(0.0);
                }
                if m.peak[c] >= 1.0 {
                    self.clip[c] = 1.0;
                    self.clip_ticks[c] = clip_ticks;
                } else if self.clip_ticks[c] > 0 {
                    self.clip_ticks[c] -= 1;
                } else {
                    self.clip[c] = 0.0;
                }
                self.vu_lin[c] += (m.rms[c] - self.vu_lin[c]) * vu_k;
                self.vu[c] = norm_db(self.vu_lin[c]);
            }
        }
    }
}

struct Running {
    uid: u64,
    engine: Engine,
    since: Instant,
}

pub struct App {
    sessions: Vec<Session>,
    cur: usize,
    sel: Option<u64>,
    running: Vec<Running>,
    pan: Vec2,
    drag_wire: Option<(u64, Pos2)>,
    templates: Vec<Template>,
    show_templates: bool,
    devices: Vec<(String, String)>,
    shot: Option<String>,
    frames: u32,
    last_saved: Instant,
    saved_snapshot: String,
    shot_requested: bool,
    meters_view: HashMap<u64, Meter>,
    disp: HashMap<(u64, u64), Disp>,
    last_tick: Instant,
    x: extras::Extras,
    timer_started: std::collections::HashSet<u64>,
    last_timer_check: Instant,
    apps: Arc<std::sync::Mutex<Vec<audio::AppInfo>>>,
    want_apps: Arc<std::sync::atomic::AtomicBool>,
}

/// The KalangoOS look: token colours, no shadows, corners of 8 (windows and panels), 6 (cards) and 4 (controls).
fn tidy_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BACK1;
    v.window_fill = BACK1;
    v.faint_bg_color = SIDE;
    v.extreme_bg_color = FIELD;
    v.code_bg_color = FIELD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.menu_corner_radius = egui::CornerRadius::same(6);
    v.hyperlink_color = THEME;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERR;
    v.override_text_color = None;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = egui::CornerRadius::same(4);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_fill = BACK1;
    v.widgets.noninteractive.weak_bg_fill = BACK1;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, DIVIDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, FORE1);
    v.widgets.inactive.bg_fill = BACK3;
    v.widgets.inactive.weak_bg_fill = BACK3;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, FORE1);
    v.widgets.hovered.bg_fill = RAISED;
    v.widgets.hovered.weak_bg_fill = RAISED;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, DIVIDER);
    v.widgets.active.bg_fill = THEME_DARK;
    v.widgets.active.weak_bg_fill = THEME_DARK;
    v.widgets.open.weak_bg_fill = RAISED;
    v.selection.bg_fill = THEME_DARK;
    v.selection.stroke = Stroke::new(1.0, THEME);
    ctx.set_visuals(v);
    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    ctx.set_global_style(style);
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, shot: Option<String>) -> Self {
        tidy_visuals(&cc.egui_ctx);
        let shot_flag = shot.is_some();
        let sessions = load_sessions();
        let snap = serde_json::to_string(&sessions).unwrap_or_default();
        // a helper keeps the list of programs that are playing sound fresh while an Application block is open
        let apps: Arc<std::sync::Mutex<Vec<audio::AppInfo>>> = Arc::new(std::sync::Mutex::new(vec![]));
        let want_apps = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if shot.is_none() {
            let (a, w) = (apps.clone(), want_apps.clone());
            std::thread::spawn(move || loop {
                if w.load(std::sync::atomic::Ordering::Relaxed) { *a.lock().unwrap() = audio::list_apps(); }
                std::thread::sleep(std::time::Duration::from_secs(2));
            });
        }
        let mut cur = 0;
        let mut sel = None;
        if shot.is_some() {
            // the screenshot shows the first session with an equalizer, with that block selected
            if let Some(i) = sessions.iter().position(|s| s.name == "Narração limpa").or_else(|| sessions.iter().position(|s| s.blocks.iter().any(|b| b.kind == "graphic_eq"))) {
                cur = i;
                sel = sessions[i].blocks.iter().find(|b| b.kind == "graphic_eq").map(|b| b.id);
            }
        }
        let mut x = extras::Extras::new(shot_flag);
        let mut show_templates = false;
        if shot_flag {
            if let Ok(w) = std::env::var("KALANGO_AUDIO_SHOW") {
                for name in w.split(',') {
                    match name.trim() {
                        "recordings" => x.show_recordings = true,
                        "presets" => x.show_presets = true,
                        "scripts" => x.show_scripts = true,
                        "mini" => x.show_mini = true,
                        "prefs" => x.show_prefs = true,
                        "about" => x.show_about = true,
                        "io" => x.show_io = true,
                        "templates" => show_templates = true,
                        "timers" => x.right_tab = 3,
                        "recs" => x.right_tab = 2,
                        "info" => x.right_tab = 5,
                        "settings" => x.right_tab = 1,
                        "home" => x.show_home = true,
                        "automations" => x.right_tab = 4,
                        "blocks" => x.right_tab = 0,
                        _ => {}
                    }
                }
            }
        }
        x.prev_sel = sel;
        App {
            sessions,
            cur,
            sel,
            running: vec![],
            pan: Vec2::new(10.0, 10.0),
            drag_wire: None,
            templates: templates(),
            show_templates,
            devices: audio::list_devices(),
            shot,
            frames: 0,
            last_saved: Instant::now(),
            saved_snapshot: snap,
            shot_requested: false,
            meters_view: HashMap::new(),
            disp: HashMap::new(),
            last_tick: Instant::now(),
            x,
            timer_started: Default::default(),
            last_timer_check: Instant::now(),
            apps,
            want_apps,
        }
    }

    /// Timers start a session when their window opens and stop it when it closes, and raise the timer automation events.
    pub fn run_timers(&mut self) {
        if self.shot.is_some() || self.last_timer_check.elapsed().as_secs() < 1 { return; }
        self.last_timer_check = Instant::now();
        let (wd, min) = engine::local_now();
        let today = engine::local_date();
        for i in 0..self.sessions.len() {
            let uid = self.sessions[i].uid;
            let mut any_open = false;
            let mut edges: Vec<(usize, bool)> = vec![];
            for (ti, t) in self.sessions[i].timers.iter().enumerate() {
                let open = extras::timer_open(t, wd, min, today);
                let was = *self.x.timer_state.get(&(uid, ti)).unwrap_or(&false);
                if open != was { edges.push((ti, open)); }
                any_open |= open;
            }
            for (ti, open) in edges {
                self.x.timer_state.insert((uid, ti), open);
                self.x.queue_event(if open { "timer_starting" } else { "timer_stopped" }, Some(uid), None);
                if !open && self.sessions[i].timers[ti].quit_sources { self.quit_sources(i); }
            }
            let running = self.is_running(uid);
            if any_open && !running { self.toggle_run(i); self.timer_started.insert(uid); }
            else if !any_open && running && self.timer_started.contains(&uid) { self.toggle_run(i); self.timer_started.remove(&uid); }
            else if !running { self.timer_started.remove(&uid); }
        }
    }

    fn is_running(&self, uid: u64) -> bool { self.running.iter().any(|r| r.uid == uid) }

    pub fn toggle_run(&mut self, idx: usize) {
        let uid = self.sessions[idx].uid;
        if let Some(p) = self.running.iter().position(|r| r.uid == uid) {
            let r = self.running.remove(p);
            let shared = r.engine.shared.clone();
            drop(r); // stopping waits until the files are finished
            let ended: Vec<engine::FileEnd> = std::mem::take(&mut *shared.events.lock().unwrap());
            self.x.on_files_ended(uid, ended);
            self.x.queue_event("session_stopped", Some(uid), None);
        } else {
            self.x.queue_event("session_starting", Some(uid), None);
            let s = &self.sessions[idx];
            // "Abrir um endereço": open the chosen program with the address when the session starts
            for b in s.blocks.iter().filter(|b| b.kind == "app_source" && b.enabled && b.p.num("url_on", 0.0) > 0.5) { audio::launch_app(&b.p.text("app", ""), &b.p.text("url", "")); }
            self.running.push(Running { uid, engine: Engine::start_with_rate(s.blocks.clone(), &s.name, s.sample_rate), since: Instant::now() });
        }
    }

    fn save_if_needed(&mut self) {
        if self.shot.is_some() || self.last_saved.elapsed().as_secs_f32() < 1.0 { return; }
        self.last_saved = Instant::now();
        // run-time counters (pause / split) change while recording, they are saved too, which is harmless
        if let Ok(t) = serde_json::to_string(&self.sessions) {
            if t != self.saved_snapshot {
                save_sessions(&self.sessions);
                self.saved_snapshot = t;
            }
        }
    }
}

fn fmt_time(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn fmt_size(b: u64) -> String {
    if b > 1_048_576 { format!("{:.1} MB", b as f64 / 1_048_576.0).replace('.', ",") } else { format!("{} KB", b / 1024) }
}

/// The engine reports states with short English codes ("recording", "error: …"); this is what the person reads.
fn state_label(state: &str) -> String {
    if let Some(e) = state.strip_prefix("error: ").or_else(|| state.strip_prefix("error:")) { return format!("erro: {}", e.trim()); }
    match state {
        "recording" => "gravando".into(),
        "paused" => "pausado".into(),
        "silent" => "em silêncio".into(),
        "finished" => "terminado".into(),
        "streaming" => "transmitindo".into(),
        "listening" => "ouvindo".into(),
        "running" => "ativo".into(),
        "error" => "erro".into(),
        s => s.to_string(),
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.frames += 1;
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
        if self.shot.is_some() {
            ctx.request_repaint();
            if self.frames >= 120 && !self.shot_requested {
                self.shot_requested = true;
                let path = self.shot.clone().unwrap_or_default();
                let c2 = ctx.clone();
                ctx.request_screenshot(move |image| {
                    if let Some(d) = std::path::Path::new(&path).parent() { let _ = std::fs::create_dir_all(d); }
                    let (w, h) = (image.width() as u32, image.height() as u32);
                    let mut raw = Vec::with_capacity((w * h * 4) as usize);
                    for px in image.pixels.iter() { raw.extend_from_slice(&[px.r(), px.g(), px.b(), 255]); }
                    if let Some(img) = image::RgbaImage::from_raw(w, h, raw) { let _ = img.save(&path); }
                    c2.send_viewport_cmd(egui::ViewportCommand::Close);
                });
            }
        }

        // push edits to running sessions and collect live meters
        for r in &self.running {
            if let Some(s) = self.sessions.iter().find(|s| s.uid == r.uid) {
                *r.engine.shared.blocks.lock().unwrap() = s.blocks.clone();
                *r.engine.shared.session_name.lock().unwrap() = s.name.clone();
            }
        }
        self.meters_view.clear();
        for r in &self.running {
            for (k, v) in r.engine.shared.meters.lock().unwrap().iter() {
                // block ids are only unique inside one session, so meters are keyed per session below
                let _ = (k, v);
            }
        }
        if self.cur >= self.sessions.len() { self.cur = self.sessions.len().saturating_sub(1); }
        if let Some(s) = self.sessions.get(self.cur) {
            if let Some(r) = self.running.iter().find(|r| r.uid == s.uid) {
                self.meters_view = r.engine.shared.meters.lock().unwrap().clone();
            }
        }

        {
            let ticks = (self.last_tick.elapsed().as_secs_f32() * 60.0).round() as u32;
            if ticks >= 1 {
                self.last_tick = Instant::now();
                let uid = self.sessions.get(self.cur).map(|s| s.uid).unwrap_or(0);
                let ids: Vec<u64> = self.meters_view.keys().copied().collect();
                for id in ids {
                    let m = self.meters_view[&id];
                    self.disp.entry((uid, id)).or_default().update(&m, ticks.min(10));
                }
                // sessions that are not running fall quiet
                let running: Vec<u64> = self.running.iter().map(|r| r.uid).collect();
                self.disp.retain(|(s, _), _| running.contains(s));
            }
        }
        self.run_timers();
        // closing the window while something is being recorded asks first
        if ctx.input(|i| i.viewport().close_requested()) && !self.x.closing && self.recordings_running() > 0 && self.shot.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.x.quit_state = 1;
        }
        if self.x.quit_now && !self.x.closing {
            for i in 0..self.sessions.len() { if self.is_running(self.sessions[i].uid) { self.toggle_run(i); } }
            self.x.quit_now = false;
            self.x.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.menu_bar(ui);
        self.notice_bar(ui);
        self.top_bar(ui);
        self.sidebar(ui);
        self.right_panel_tabs(ui);
        self.canvas(ui);
        self.extra_windows(&ctx);
        self.tick_extras(&ctx);
        self.save_if_needed();
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        egui::Panel::top("top").exact_size(44.0).frame(egui::Frame::new().fill(BACK2).inner_margin(egui::Margin::symmetric(12, 6))).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if self.sessions.is_empty() {
                    ui.label(egui::RichText::new("Kalango Audio").strong().size(16.0).color(FORE1));
                    return;
                }
                let idx = self.cur;
                let uid = self.sessions[idx].uid;
                let running = self.is_running(uid);
                let (label, col) = if running { ("■  Parar", LIVE) } else { ("▶  Iniciar", GREEN) };
                if ui.add(egui::Button::new(egui::RichText::new(label).strong().color(ON_THEME)).fill(col).min_size(Vec2::new(96.0, 28.0))).clicked() {
                    self.toggle_run(idx);
                }
                ui.add_space(8.0);
                ui.add(egui::TextEdit::singleline(&mut self.sessions[idx].name).font(FontId::proportional(17.0)).desired_width(280.0).frame(egui::Frame::NONE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(r) = self.running.iter().find(|r| r.uid == uid) {
                        ui.label(egui::RichText::new(format!("● AO VIVO  {}", fmt_time(r.since.elapsed().as_secs_f64()))).color(LIVE).strong());
                        let load = r.engine.shared.load.load(std::sync::atomic::Ordering::Relaxed) as f32 / 10.0;
                        let late = r.engine.shared.overruns.load(std::sync::atomic::Ordering::Relaxed);
                        ui.label(egui::RichText::new(format!("Carga de áudio {load:.0}%{}", if late > 0 { format!(" · {late} atrasos") } else { String::new() })).small().color(if load > 80.0 || late > 0 { ERR } else { FORE2 }))
                            .on_hover_text("Quanto do tempo disponível o motor de som está usando. Acima de 100% o som pode falhar; desligue blocos pesados se isso acontecer.");
                    } else {
                        ui.label(egui::RichText::new("Parado").color(FORE2));
                    }
                    ui.add_space(10.0);
                    let mut manual = self.sessions[idx].manual;
                    if ui.toggle_value(&mut manual, "Ligar à mão").on_hover_text("Liga os blocos à mão em vez de automaticamente").changed() { self.sessions[idx].manual = manual; }
                });
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        egui::Panel::left("sessions").exact_size(210.0).frame(egui::Frame::new().fill(SIDE).inner_margin(egui::Margin::same(10))).show(ui, |ui| {
            ui.label(egui::RichText::new("SESSÕES").small().color(FORE2));
            ui.add_space(4.0);
            let mut pick = None;
            let mut run_idx: Option<usize> = None;
            let mut dup_idx: Option<usize> = None;
            let mut del_idx: Option<usize> = None;
            egui::ScrollArea::vertical().max_height(ui.available_height() - 70.0).show(ui, |ui| {
                for (i, s) in self.sessions.iter().enumerate() {
                    let running = self.running.iter().any(|r| r.uid == s.uid);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                    let selected = i == self.cur;
                    if selected { ui.painter().rect_filled(rect, 6.0, THEME_DARK); }
                    else if resp.hovered() { ui.painter().rect_filled(rect, 6.0, BACK3); }
                    let dot = if running { LIVE } else { DIM };
                    ui.painter().circle_filled(Pos2::new(rect.left() + 14.0, rect.center().y), 5.0, dot);
                    let name: String = if s.name.chars().count() > 16 { format!("{}…", s.name.chars().take(15).collect::<String>()) } else { s.name.clone() };
                    ui.painter().text(Pos2::new(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, name, FontId::proportional(14.0), FORE1);
                    // little start / stop button on every row
                    let br = Rect::from_center_size(Pos2::new(rect.right() - 16.0, rect.center().y), Vec2::splat(24.0));
                    let bresp = ui.interact(br, ui.make_persistent_id(("rowbtn", s.uid)), Sense::click());
                    if bresp.hovered() || running { ui.painter().circle_filled(br.center(), 11.0, if running { LIVE } else { RAISED }); }
                    ui.painter().text(br.center(), Align2::CENTER_CENTER, if running { "■" } else { "▶" }, FontId::proportional(11.0), if running { ON_THEME } else { FORE1 });
                    if bresp.on_hover_text(if running { "Parar esta sessão" } else { "Iniciar esta sessão" }).clicked() { run_idx = Some(i); }
                    else if resp.clicked() { pick = Some(i); }
                    resp.context_menu(|ui| {
                        if ui.button("Duplicar").clicked() { dup_idx = Some(i); ui.close(); }
                        if ui.button("Apagar").clicked() { del_idx = Some(i); ui.close(); }
                    });
                }
            });
            if let Some(i) = pick { self.cur = i; self.sel = None; self.pan = Vec2::new(10.0, 10.0); }
            if let Some(i) = run_idx { self.toggle_run(i); }
            if let Some(i) = dup_idx { self.cur = i; self.duplicate_session(); }
            if let Some(i) = del_idx { self.cur = i; self.delete_session(); }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                if ui.add_enabled(!self.sessions.is_empty(), egui::Button::new("−  Apagar sessão").min_size(Vec2::new(ui.available_width(), 26.0))).clicked() {
                    let uid = self.sessions[self.cur].uid;
                    self.running.retain(|r| r.uid != uid);
                    self.sessions.remove(self.cur);
                    self.sel = None;
                }
                if ui.add(egui::Button::new(egui::RichText::new("+  Nova sessão").color(ON_THEME)).fill(THEME).min_size(Vec2::new(ui.available_width(), 28.0))).clicked() {
                    self.show_templates = true;
                }
            });
        });
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BACK1)).show(ui, |ui| {
            let full = ui.available_rect_before_wrap();
            let (_, bg) = ui.allocate_exact_size(full.size(), Sense::click_and_drag());
            let painter = ui.painter_at(full);
            // dotted grid
            let origin = full.min + self.pan;
            let mut gx = full.left() + self.pan.x.rem_euclid(UNIT);
            while gx < full.right() {
                let mut gy = full.top() + self.pan.y.rem_euclid(UNIT);
                while gy < full.bottom() {
                    painter.circle_filled(Pos2::new(gx, gy), 0.8, DIVIDER);
                    gy += UNIT;
                }
                gx += UNIT;
            }
            if self.sessions.is_empty() {
                painter.text(full.center(), Align2::CENTER_CENTER, "Nenhuma sessão ainda. Clique em “Nova sessão” para começar.", FontId::proportional(16.0), FORE2);
                return;
            }
            {
                let manual = self.sessions[self.cur].manual;
                let msg = if manual { "Ligação à mão: arraste do ponto à direita de um bloco até outro bloco. Clique no × de um fio para tirá-lo." } else { "Os blocos se ligam sozinhos. Ative “Ligar à mão” para ligá-los você mesmo." };
                painter.text(full.left_top() + Vec2::new(14.0, 12.0), Align2::LEFT_TOP, msg, FontId::proportional(11.5), if manual { WARN } else { FORE2 });
            }
            if bg.dragged() && self.drag_wire.is_none() { self.pan += bg.drag_delta(); }
            if bg.clicked() { self.sel = None; }

            let cur = self.cur;
            let uid = self.sessions[cur].uid;
            let rec_status = self.running.iter().find(|r| r.uid == uid).map(|r| r.engine.shared.rec.lock().unwrap().clone()).unwrap_or_default();
            let live = self.is_running(uid);
            let disps: HashMap<u64, Disp> = self.disp.iter().filter(|((s, _), _)| *s == uid).map(|((_, b), d)| (*b, *d)).collect();

            let rects: HashMap<u64, Rect> = self.sessions[cur].blocks.iter()
                .map(|b| (b.id, Rect::from_min_size(origin + Vec2::new(b.x * UNIT, b.y * UNIT), Vec2::new(BW, BH)))).collect();

            // wires
            let mut cut: Option<(u64, u64)> = None;
            for b in &self.sessions[cur].blocks {
                for o in &b.outs {
                    if let (Some(a), Some(c)) = (rects.get(&b.id), rects.get(o)) {
                        let p0 = Pos2::new(a.right(), a.center().y);
                        let p1 = Pos2::new(c.left(), c.center().y);
                        let dx = ((p1.x - p0.x).abs() * 0.5).max(30.0);
                        let col = if live && b.enabled { LIVE } else { THEME };
                        let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                            [p0, p0 + Vec2::new(dx, 0.0), p1 - Vec2::new(dx, 0.0), p1], false, Color32::TRANSPARENT, Stroke::new(2.5, col));
                        painter.add(shape);
                        let mid = Pos2::new((p0.x + p1.x) * 0.5, (p0.y + p1.y) * 0.5);
                        let hot = ui.input(|i| i.pointer.hover_pos()).map(|p| p.distance(mid) < 10.0).unwrap_or(false);
                        if hot {
                            painter.circle_filled(mid, 8.0, ERR);
                            painter.text(mid, Align2::CENTER_CENTER, "×", FontId::proportional(13.0), ON_THEME);
                            if ui.input(|i| i.pointer.primary_clicked()) { cut = Some((b.id, *o)); }
                        }
                    }
                }
            }
            if let Some((a, c)) = cut { self.sessions[cur].disconnect(a, c); }

            // blocks
            let ids: Vec<u64> = self.sessions[cur].blocks.iter().map(|b| b.id).collect();
            let mut move_to: Option<(u64, Vec2)> = None;
            let mut snap: Option<u64> = None;
            let mut connect: Option<(u64, u64)> = None;
            let mut remove: Option<u64> = None;
            let mut dup: Option<u64> = None;
            for id in ids {
                let rect = rects[&id];
                if !rect.intersects(full.expand(60.0)) { continue; }
                let b = self.sessions[cur].block(id).unwrap().clone();
                let resp = ui.interact(rect, ui.id().with(("blk", id)), Sense::click_and_drag());
                if resp.clicked() || resp.drag_started() { self.sel = Some(id); }
                if resp.dragged() { move_to = Some((id, resp.drag_delta())); }
                if resp.drag_stopped() { snap = Some(id); }
                resp.context_menu(|ui| {
                    if ui.button(if b.enabled { "Desligar" } else { "Ligar" }).clicked() {
                        if let Some(bb) = self.sessions[cur].blocks.iter_mut().find(|x| x.id == id) { bb.enabled = !bb.enabled; }
                        ui.close();
                    }
                    if ui.button("Duplicar").clicked() { dup = Some(id); ui.close(); }
                    if ui.button("Apagar").clicked() { remove = Some(id); ui.close(); }
                });
                let selected = self.sel == Some(id);
                draw_block(&painter, rect, &b, selected, live, disps.get(&id), rec_status.get(&id));

                // ports
                let info = kind_info(&b.kind);
                let has_out = !matches!(info.map(|i| i.id), Some("rtmp_stream") | Some("radio_stream"));
                if !is_source(&b.kind) {
                    painter.circle(Pos2::new(rect.left(), rect.center().y), 5.0, BACK2, Stroke::new(2.0, THEME));
                }
                if has_out {
                    let port = Pos2::new(rect.right(), rect.center().y);
                    painter.circle(port, 5.0, THEME, Stroke::new(2.0, FORE1));
                    let pr = ui.interact(Rect::from_center_size(port, Vec2::splat(18.0)), ui.id().with(("port", id)), Sense::drag());
                    if pr.drag_started() { self.drag_wire = Some((id, port)); }
                    if pr.dragged() {
                        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) { self.drag_wire = Some((id, p)); }
                    }
                    if pr.drag_stopped() {
                        if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                            if let Some((tid, _)) = rects.iter().find(|(k, r)| **k != id && r.contains(p)) { connect = Some((id, *tid)); }
                        }
                        self.drag_wire = None;
                    }
                }
                // the Volume slider lives right on the block
                if b.kind == "volume" {
                    let sl = Rect::from_min_size(rect.left_top() + Vec2::new(10.0, 58.0), Vec2::new(BW - 20.0, 18.0));
                    let mut g = b.p.num("gain", 1.0) as f32;
                    if ui.put(sl, egui::Slider::new(&mut g, 0.0..=1.0).show_value(false)).changed() {
                        if let Some(bb) = self.sessions[cur].blocks.iter_mut().find(|x| x.id == id) { bb.p.set("gain", g as f64); }
                    }
                }
            }
            if let Some((id, pos)) = self.drag_wire {
                if let Some(a) = rects.get(&id) {
                    let p0 = Pos2::new(a.right(), a.center().y);
                    let dx = ((pos.x - p0.x).abs() * 0.5).max(30.0);
                    painter.add(egui::epaint::CubicBezierShape::from_points_stroke([p0, p0 + Vec2::new(dx, 0.0), pos - Vec2::new(dx, 0.0), pos], false, Color32::TRANSPARENT, Stroke::new(2.0, FORE2)));
                }
            }
            if let Some((id, d)) = move_to {
                if let Some(bb) = self.sessions[cur].blocks.iter_mut().find(|x| x.id == id) {
                    bb.x += d.x / UNIT;
                    bb.y += d.y / UNIT;
                }
            }
            // blocks settle on the grid when let go, so chains line up
            if let Some(id) = snap { if let Some(bb) = self.sessions[cur].blocks.iter_mut().find(|x| x.id == id) { bb.x = bb.x.round(); bb.y = bb.y.round(); } }
            if let Some((a, c)) = connect { self.sessions[cur].connect(a, c); }
            if let Some(id) = remove { self.remove_block_smart(id); }
            if let Some(id) = dup {
                if let Some(src) = self.sessions[cur].block(id).cloned() {
                    let nid = self.sessions[cur].add_block(&src.kind, src.x + 1.0, src.y + 4.0);
                    if let Some(nb) = self.sessions[cur].blocks.iter_mut().find(|x| x.id == nid) { nb.p = src.p.clone(); }
                    self.sel = Some(nid);
                }
            }
            if self.sel.is_some() && ui.input(|i| i.key_pressed(egui::Key::Delete)) && !ctx.egui_wants_keyboard_input() {
                if let Some(id) = self.sel { self.remove_block_smart(id); }
            }
        });
    }
}

/// Block heads by category, from the token palette (cor/teal, cor/gold, cor/accent, cor/blue, cor/grey_tile);
/// their text is drawn in ON_THEME.
fn kind_color(c: Cat) -> Color32 {
    match c {
        Cat::Source => GREEN,
        Cat::Output => WARN,
        Cat::Effect => THEME,
        Cat::Meter => PEAK,
        Cat::Advanced => Color32::from_rgb(0x9C, 0x9D, 0xAE),
    }
}

fn meter_bar(p: &egui::Painter, r: Rect, peak: f32, rms: f32, hold: f32, clip: f32) {
    p.rect_filled(r, 2.0, FIELD);
    let rr = Rect::from_min_size(r.min, Vec2::new(r.width() * rms, r.height()));
    p.rect_filled(rr, 2.0, GREEN);
    let pr = Rect::from_min_size(r.min, Vec2::new(r.width() * peak, r.height() * 0.45));
    p.rect_filled(pr, 1.0, PEAK);
    if hold > 0.0 {
        let x = r.left() + r.width() * hold;
        p.line_segment([Pos2::new(x, r.top()), Pos2::new(x, r.bottom())], Stroke::new(2.0, WARN));
    }
    if clip > 0.3 { p.circle_filled(Pos2::new(r.right() + 5.0, r.center().y), 2.5, ERR); }
}

fn draw_block(p: &egui::Painter, rect: Rect, b: &Block, selected: bool, live: bool, m: Option<&Disp>, rec: Option<&engine::RecStatus>) {
    let info = kind_info(&b.kind);
    let cat = info.map(|i| i.cat).unwrap_or(Cat::Effect);
    let dim = if b.enabled { 1.0 } else { 0.45 };
    let fade = |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (255.0 * dim) as u8);
    p.rect_filled(rect, 6.0, fade(BACK3));
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 22.0));
    p.rect_filled(head, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, fade(kind_color(cat)));
    let title = info.map(|i| i.title).unwrap_or("?");
    // long pt-BR titles are cut so they stay inside the block's head
    let title: String = if title.chars().count() > 16 { format!("{}…", title.chars().take(15).collect::<String>().trim_end()) } else { title.to_string() };
    p.text(head.center(), Align2::CENTER_CENTER, title, FontId::proportional(11.5), ON_THEME);
    let stroke = if selected { Stroke::new(2.0, FORE1) } else { Stroke::new(1.0, BORDER) };
    p.rect_stroke(rect, 6.0, stroke, StrokeKind::Inside);

    let body = Rect::from_min_max(rect.left_top() + Vec2::new(8.0, 26.0), rect.right_bottom() - Vec2::new(8.0, 20.0));
    let line = |dy: f32, t: &str, c: Color32| {
        p.text(body.left_top() + Vec2::new(0.0, dy), Align2::LEFT_TOP, t, FontId::proportional(11.5), c);
    };
    match b.kind.as_str() {
        "volume" => {
            let total = crate::dsp::gain_total(&b.p);
            p.text(body.left_top() + Vec2::new(0.0, 2.0), Align2::LEFT_TOP, format!("{}%", (total * 100.0).round() as i32), FontId::proportional(17.0), THEME);
            let od = b.p.num("boost", 1.0) as i32;
            if od > 1 { p.text(body.right_top() + Vec2::new(0.0, 5.0), Align2::RIGHT_TOP, format!("{od}×"), FontId::proportional(12.0), WARN); }
        }
        "recorder" => {
            let fmt = b.p.text("format", "WAV");
            line(0.0, &fmt, FORE1);
            let st = rec.cloned().unwrap_or_default();
            let t = if live { format!("{}  {}", fmt_time(st.seconds), fmt_size(st.bytes)) } else { "00:00:00".to_string() };
            line(15.0, &t, FORE2);
            if live && st.state == "recording" { p.circle_filled(rect.right_top() + Vec2::new(-12.0, 11.0), 4.0, LIVE); }
            if live && !st.state.is_empty() && st.state != "recording" { line(30.0, &state_label(&st.state).chars().take(16).collect::<String>(), if st.state.starts_with("error") { ERR } else { WARN }); }
        }
        "device_input" | "device_output" | "system_sound" | "app_source" => {
            let t = match b.kind.as_str() {
                "app_source" => audio::app_label(&b.p.text("app", "Web Browser")),
                _ => audio::device_label(&b.p.text("device", "default")),
            };
            let t: String = t.chars().take(17).collect();
            line(0.0, &t, FORE1);
            if b.kind == "device_output" { let g = (b.p.num("gain", 1.0) * 100.0).round() as i32; line(15.0, &format!("{g}%"), FORE2); }
        }
        "test_tone" => { line(0.0, &format!("{} Hz", b.p.num("freq", 440.0).round() as i32), FORE1); }
        "graphic_eq" => {
            let bw = body.width() / 10.0;
            for i in 0..10 {
                let v = b.p.num(&format!("band{i}"), 0.5) as f32;
                let h = (v - 0.5) * (body.height() - 4.0) * 1.6;
                let x = body.left() + bw * i as f32 + bw * 0.5;
                let cy = body.center().y;
                p.line_segment([Pos2::new(x, cy), Pos2::new(x, cy - h)], Stroke::new(3.0, THEME));
            }
            p.line_segment([Pos2::new(body.left(), body.center().y), Pos2::new(body.right(), body.center().y)], Stroke::new(0.5, FORE2));
        }
        "balance" | "pan" => {
            let v = b.p.num(if b.kind == "pan" { "pan" } else { "balance" }, 0.0) as f32;
            let r = Rect::from_center_size(Pos2::new(body.center().x, body.top() + 14.0), Vec2::new(body.width(), 6.0));
            p.rect_filled(r, 3.0, BACK2);
            p.circle_filled(Pos2::new(r.center().x + v * r.width() * 0.5, r.center().y), 6.0, THEME);
            line(24.0, &if v.abs() < 0.01 { "Centro".to_string() } else if v < 0.0 { format!("E {}", (-v * 100.0) as i32) } else { format!("D {}", (v * 100.0) as i32) }, FORE2);
        }
        "tone_control" => {
            line(0.0, &format!("Graves {:+.0} dB", (b.p.num("bass", 0.5) - 0.5) * 24.0), FORE1);
            line(15.0, &format!("Agudos {:+.0} dB", (b.p.num("treble", 0.5) - 0.5) * 24.0), FORE1);
        }
        "voice_denoise" => line(0.0, if b.p.num("mono", 0.0) > 0.5 { "Mono" } else { "Estéreo" }, FORE1),
        "convolution" => {
            let ir = b.p.text("ir", "");
            let n = ir.rsplit('/').next().unwrap_or("").to_string();
            line(0.0, &if n.is_empty() { "Sem impulso".to_string() } else { n.chars().take(16).collect() }, FORE1);
        }
        "mixer" => line(0.0, &format!("{} faixas", (0..5).filter(|i| b.p.num(&format!("m{i}"), 0.0) < 0.5).count()), FORE1),
        "ducking" => line(0.0, &format!("{:.0} dB · {:.0}%", b.p.num("threshold", -30.0), b.p.num("ducked", 0.4) * 100.0), FORE1),
        "noise_reduction" => line(0.0, &format!("−{:.0} dB", b.p.num("reduction", 6.0)), FORE1),
        "delay" => line(0.0, &format!("{} ms", b.p.num("delay", 0.0).floor() as i32), FORE1),
        "time_shift" => { let lag = rec.map(|r| r.seconds).unwrap_or(0.0); line(0.0, &if lag < 0.05 { "Ao vivo".to_string() } else { format!("−{:.1} s", lag).replace('.', ",") }, if lag < 0.05 { GREEN } else { WARN }); }
        "source_switch" => line(0.0, &format!("Entrada {}", b.p.num("switch", 0.0) as i32 + 1), FORE1),
        "parametric_eq" => line(0.0, &format!("{} bandas", b.p.num("bands", 3.0) as i32), FORE1),
        "high_pass" | "band_pass" | "high_shelf" | "low_shelf" => line(0.0, &format!("{} Hz", b.p.num("freq", 1000.0).round() as i32), FORE1),
        "radio_stream" | "rtmp_stream" | "transcribe" => {
            let st = rec.cloned().unwrap_or_default();
            if live && !st.state.is_empty() {
                let col = if st.state.starts_with("error") { ERR } else { GREEN };
                line(0.0, &state_label(&st.state).chars().take(17).collect::<String>(), col);
                if !st.state.starts_with("error") { line(15.0, &fmt_time(st.seconds), FORE2); }
            } else { line(0.0, if b.kind == "radio_stream" { "Icecast" } else if b.kind == "rtmp_stream" { "Ao vivo (RTMP)" } else { "Fala em texto" }, FORE1); }
        }
        "needle_meter" => {
            let db = m.map(|d| d.vu[0].max(d.vu[1]) * 60.0 - 60.0).unwrap_or(-70.0);
            let ang = crate::dsp::vu_angle(db).to_radians();
            let pivot = Pos2::new(body.center().x, body.bottom() + 4.0);
            let len = body.height() + 2.0;
            p.line_segment([pivot, pivot + Vec2::new(ang.sin() * len, -ang.cos() * len)], Stroke::new(2.0, WARN));
            p.circle_filled(pivot, 3.0, FORE2);
        }
        "low_pass" => line(0.0, &format!("{} Hz", b.p.num("freq", 10000.0).round() as i32), FORE1),
        "hum_removal" => line(0.0, &format!("{} Hz × {}", b.p.num("base", 50.0).round() as i32, b.p.num("amount", 3.0).round() as i32), FORE1),
        "channels" => line(0.0, TWEAK_MODES[(b.p.num("mode", 0.0) as usize).min(TWEAK_MODES.len() - 1)], FORE1),
        "compressor" | "loudness" => line(0.0, &format!("Força {}%", (b.p.num("amount", 0.0) * 100.0).round() as i32), FORE1),
        _ => {
            if let Some(i) = info { if !i.working { line(0.0, "(só repassa)", FORE2); } }
        }
    }
    // level meters along the bottom
    if let Some(m) = m {
        let w = rect.width() - 22.0;
        let r1 = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.bottom() - 16.0), Vec2::new(w, 5.0));
        let r2 = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.bottom() - 9.0), Vec2::new(w, 5.0));
        meter_bar(p, r1, m.peak[0], m.rms[0], m.hold[0], m.clip[0]);
        meter_bar(p, r2, m.peak[1], m.rms[1], m.hold[1], m.clip[1]);
    } else {
        let w = rect.width() - 16.0;
        for k in 0..2 {
            p.rect_filled(Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.bottom() - 16.0 + 7.0 * k as f32), Vec2::new(w, 5.0)), 2.0, FIELD);
        }
    }
}

/// Shown in the compressor's mode list when the sliders match none of the ready-made modes.
const CUSTOM_MODE: &str = "Personalizado";

const TWEAK_MODES: [&str; 7] = ["Normal", "Trocar esquerda e direita", "Esquerda nos dois lados", "Direita nos dois lados", "Juntar em mono", "Calar a esquerda", "Calar a direita"];

fn slider_row(ui: &mut egui::Ui, label: &str, p: &mut Params, key: &str, range: std::ops::RangeInclusive<f64>, default: f64, suffix: &str) {
    let mut v = p.num(key, default);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(FORE2));
        if ui.add(egui::Slider::new(&mut v, range).suffix(suffix)).changed() { p.set(key, v); }
    });
}

fn device_combo(ui: &mut egui::Ui, id: &str, p: &mut Params, devices: &[(String, String)]) {
    let cur = p.text("device", "default");
    let label = devices.iter().find(|d| d.0 == cur).map(|d| d.1.clone()).unwrap_or(cur.clone());
    egui::ComboBox::from_id_salt(id).width(230.0).selected_text(label).show_ui(ui, |ui| {
        for (dev, name) in devices {
            if ui.selectable_label(*dev == cur, name).clicked() { p.set_text("device", dev); }
        }
    });
}

fn block_settings(ui: &mut egui::Ui, b: &mut Block, devices: &[(String, String)], apps_now: &[audio::AppInfo], ups: &[(u64, String)], rec: Option<engine::RecStatus>, err: Option<String>) {
    let kind = b.kind.clone();
    let p = &mut b.p;
    if let Some(e) = err { ui.label(egui::RichText::new(e).color(ERR)); }
    match kind.as_str() {
        "volume" => {
            slider_row(ui, "Volume", p, "gain", 0.0..=1.0, 1.0, "");
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Reforço").color(FORE2));
                let cur = p.num("boost", 1.0) as i32;
                for (v, t) in [(1, "Sem"), (2, "2×"), (3, "3×"), (4, "4×")] {
                    if ui.selectable_label(cur == v, t).clicked() { p.set("boost", v as f64); }
                }
            });
            let mut mute = p.num("muted", 0.0) > 0.5;
            if ui.checkbox(&mut mute, "Silenciar").changed() { p.set("muted", if mute { 1.0 } else { 0.0 }); }
            let total = crate::dsp::gain_total(p);
            ui.label(egui::RichText::new(format!("Ganho total: {}%  (de 0% a {}%)", (total * 100.0).round() as i32, 100 * p.num("boost", 1.0) as i32)).small().color(FORE2));
        }
        "device_input" | "system_sound" | "app_source" => {
            if kind == "app_source" {
                app_source_settings(ui, p, apps_now);
            } else {
                ui.label(egui::RichText::new(if kind == "system_sound" { "Dispositivo de captura (escolha uma fonte “monitor” para ouvir o sistema)" } else { "Dispositivo" }).color(FORE2));
                device_combo(ui, "dev", p, devices);
            }
        }
        "device_output" => {
            ui.label(egui::RichText::new("Dispositivo").color(FORE2));
            device_combo(ui, "dev", p, devices);
            slider_row(ui, "Volume", p, "gain", 0.0..=1.0, 1.0, "");
            let mut mute = p.num("muted", 0.0) > 0.5;
            if ui.checkbox(&mut mute, "Sem som").changed() { p.set("muted", if mute { 1.0 } else { 0.0 }); }
        }
        "test_tone" => {
            slider_row(ui, "Frequência", p, "freq", 50.0..=4000.0, 440.0, " Hz");
            slider_row(ui, "Nível", p, "level", 0.0..=1.0, 0.25, "");
        }
        "graphic_eq" => {
            ui.horizontal(|ui| {
                for i in 0..10 {
                    ui.vertical(|ui| {
                        let k = format!("band{i}");
                        let mut v = p.num(&k, 0.5) as f32;
                        let db = (v - 0.5) * 24.0;
                        ui.label(egui::RichText::new(format!("{:+.0}", db)).small().color(THEME));
                        if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).vertical().show_value(false)).changed() { p.set(&k, v as f64); }
                        let f = EQ_FREQS[i];
                        ui.label(egui::RichText::new(if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{}", f) }).small().color(FORE2));
                    });
                }
            });
            if ui.small_button("Zerar").clicked() { for i in 0..10 { p.set(&format!("band{i}"), 0.5); } }
        }
        "balance" => slider_row(ui, "Equilíbrio", p, "balance", -1.0..=1.0, 0.0, ""),
        "pan" => slider_row(ui, "Posição", p, "pan", -1.0..=1.0, 0.0, ""),
        "tone_control" => {
            slider_row(ui, "Graves", p, "bass", 0.0..=1.0, 0.5, "");
            slider_row(ui, "Agudos", p, "treble", 0.0..=1.0, 0.5, "");
            ui.label(egui::RichText::new("No meio não muda nada; nas pontas, ±12 dB.").small().color(FORE2));
        }
        "low_pass" => {
            // steps of roughly 1 to 5% of the frequency, so each notch of the slider sounds about the same size
            let mut f = p.num("freq", 10000.0);
            let step = if f < 500.0 { 5.0 } else if f < 5000.0 { 50.0 } else { 250.0 };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Corte").color(FORE2));
                if ui.add(egui::Slider::new(&mut f, 100.0..=20000.0).suffix(" Hz").step_by(step)).changed() { p.set("freq", f); }
            });
        }
        "compressor" | "loudness" => {
            if kind == "compressor" {
                // our ready-made modes (extras::COMPRESSOR_MODES); any other mix of the sliders shows as "Personalizado"
                let (pu, sp, mk) = (p.num("amount", 1.0), p.num("speed", 0.0), p.num("makeup", 1.0));
                let cur = extras::COMPRESSOR_MODES.iter().find(|m| (m.1 - pu).abs() < 0.005 && (m.2 - sp).abs() < 0.005 && (m.3 - mk).abs() < 0.005).map(|m| m.0).unwrap_or(CUSTOM_MODE);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Modo").color(FORE2));
                    egui::ComboBox::from_id_salt("cmode").selected_text(cur).show_ui(ui, |ui| {
                        for m in extras::COMPRESSOR_MODES { if ui.selectable_label(cur == m.0, m.0).clicked() { p.set("amount", m.1); p.set("speed", m.2); p.set("makeup", m.3); } }
                        ui.add_enabled(false, egui::Button::selectable(cur == CUSTOM_MODE, CUSTOM_MODE));
                    });
                });
            }
            slider_row(ui, "Intensidade", p, "amount", 0.0..=1.0, if kind == "loudness" { 0.0 } else { 1.0 }, "");
            slider_row(ui, "Velocidade", p, "speed", 0.0..=1.0, 0.0, "");
            slider_row(ui, "Compensação", p, "makeup", 1.0..=4.0, 1.0, "×");
        }
        "channels" => {
            let cur = (p.num("mode", 0.0) as usize).min(TWEAK_MODES.len() - 1);
            egui::ComboBox::from_id_salt("tweak").width(230.0).selected_text(TWEAK_MODES[cur]).show_ui(ui, |ui| {
                for (i, t) in TWEAK_MODES.iter().enumerate() { if ui.selectable_label(cur == i, *t).clicked() { p.set("mode", i as f64); } }
            });
        }
        "hum_removal" => {
            slider_row(ui, "Frequência da rede", p, "base", 40.0..=70.0, 50.0, " Hz");
            slider_row(ui, "Harmônicos", p, "amount", 1.0..=8.0, 3.0, "");
        }
        "click_removal" => slider_row(ui, "Sensibilidade", p, "sens", 1.0..=6.0, 3.0, ""),
        "mixer" => {
            ui.label(egui::RichText::new("As faixas seguem a ordem em que os blocos foram ligados (até 5).").small().color(FORE2));
            for i in 0..5 {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        let name = ups.get(i).map(|x| x.1.clone()).unwrap_or_else(|| format!("Faixa {}", i + 1));
                        ui.add_sized([80.0, 18.0], egui::Label::new(egui::RichText::new(name).color(if ups.get(i).is_some() { FORE1 } else { FORE2 })));
                        let mut g = p.num(&format!("g{i}"), 1.0) as f32;
                        if ui.add(egui::Slider::new(&mut g, 0.0..=1.0).show_value(false)).changed() { p.set(&format!("g{i}"), g as f64); }
                        let mut m = p.num(&format!("m{i}"), 0.0) > 0.5;
                        if ui.toggle_value(&mut m, "M").on_hover_text("Calar esta faixa").changed() { p.set(&format!("m{i}"), if m { 1.0 } else { 0.0 }); }
                        let mut s = p.num(&format!("s{i}"), 0.0) > 0.5;
                        if ui.toggle_value(&mut s, "S").on_hover_text("Ouvir só esta faixa").changed() { p.set(&format!("s{i}"), if s { 1.0 } else { 0.0 }); }
                    });
                    ui.horizontal(|ui| {
                        let mut bal = p.num(&format!("b{i}"), 0.0) as f32;
                        ui.label(egui::RichText::new("Equil.").small().color(FORE2));
                        if ui.add(egui::Slider::new(&mut bal, -1.0..=1.0).show_value(false)).changed() { p.set(&format!("b{i}"), bal as f64); }
                        let mut pan = p.num(&format!("p{i}"), 0.0) as f32;
                        ui.label(egui::RichText::new("Posição").small().color(FORE2));
                        if ui.add(egui::Slider::new(&mut pan, -1.0..=1.0).show_value(false)).changed() { p.set(&format!("p{i}"), pan as f64); }
                    });
                    ui.horizontal(|ui| {
                        let faded = p.num(&format!("fo{i}"), 0.0) > 0.5;
                        if ui.small_button(if faded { "↑ Subir aos poucos (100%)" } else { "↓ Baixar aos poucos (0%)" }).clicked() { p.set(&format!("fo{i}"), if faded { 0.0 } else { 1.0 }); }
                    });
                });
            }
        }
        "ducking" => {
            ui.label(egui::RichText::new("Fonte principal (passa à frente quando soa alto o bastante)").color(FORE2));
            let cur = p.num("overlay", 0.0) as u64;
            let label = ups.iter().find(|x| x.0 == cur).or(ups.first()).map(|x| x.1.clone()).unwrap_or_else(|| "Ligue duas fontes".into());
            egui::ComboBox::from_id_salt("overlay").width(230.0).selected_text(label).show_ui(ui, |ui| {
                for (id, t) in ups { if ui.selectable_label(*id == cur, t).clicked() { p.set("overlay", *id as f64); } }
            });
            slider_row(ui, "Limiar", p, "threshold", -60.0..=0.0, -30.0, " dB");
            let mut d = (p.num("ducked", 0.4) * 100.0) as f32;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("As outras caem para").color(FORE2));
                if ui.add(egui::Slider::new(&mut d, 0.0..=100.0).suffix(" %")).changed() { p.set("ducked", d as f64 / 100.0); }
            });
            ui.label(egui::RichText::new("Quando a fonte principal passa do limiar, as outras baixam; quando ela se cala, as outras voltam ao volume cheio.").small().color(FORE2));
        }
        "noise_reduction" => {
            slider_row(ui, "Redução", p, "reduction", 0.0..=40.0, 6.0, " dB");
            let learned = rec.as_ref().map(|r| r.seconds).unwrap_or(0.0);
            let learning = rec.as_ref().map(|r| r.state == "noise_reduction").unwrap_or(false) && learned > 0.0 && learned < 3.0;
            if ui.button(if learning { "Parar de aprender" } else { "Aprender o ruído" }).clicked() { let n = p.num("learn_seq", 0.0); p.set("learn_seq", n + 1.0); }
            ui.label(egui::RichText::new(if learned > 0.0 { format!("{} s de ruído aprendidos", format!("{learned:.1}").replace('.', ",")) } else { "Nenhum ruído aprendido".to_string() }).color(FORE1));
            ui.label(egui::RichText::new("Inicie a sessão e clique em “Aprender o ruído” enquanto só o ruído estiver tocando. São usados 3 segundos de som.").small().color(FORE2));
        }
        "ladspa_plugin" => {
            let plugins = crate::ladspa::cached();
            let cur = p.text("plugin", "");
            let label = plugins.iter().find(|x| x.key() == cur).map(|x| x.name.clone()).unwrap_or_else(|| if cur.is_empty() { "Escolha um plug-in".to_string() } else { "(não encontrado)".to_string() });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("plugpick").width(220.0).selected_text(label).show_ui(ui, |ui| {
                    if plugins.is_empty() { ui.label(egui::RichText::new("Nenhum plug-in LADSPA encontrado").small().color(FORE2)); }
                    for x in &plugins { if ui.selectable_label(x.key() == cur, format!("{}  ·  {}", x.name, x.maker)).clicked() { p.set_text("plugin", &x.key()); } }
                });
                if ui.small_button("Procurar de novo").clicked() { crate::ladspa::rescan(); }
            });
            if let Some(info) = plugins.iter().find(|x| x.key() == cur) {
                for c in &info.controls {
                    let key = format!("c{}", c.port);
                    let mut v = p.num(&key, c.default as f64);
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 18.0], egui::Label::new(egui::RichText::new(&c.name).small().color(FORE2)));
                        if c.toggle {
                            let mut on = v > 0.5;
                            if ui.checkbox(&mut on, "").changed() { p.set(&key, if on { 1.0 } else { 0.0 }); }
                        } else {
                            let mut s = egui::Slider::new(&mut v, c.min as f64..=(c.max.max(c.min + 1e-6)) as f64);
                            if c.log && c.min > 0.0 { s = s.logarithmic(true); }
                            if c.integer { s = s.integer(); }
                            if ui.add(s).changed() { p.set(&key, v); }
                        }
                    });
                }
            }
            match rec.as_ref().map(|r| r.state.clone()) {
                Some(s) if s.starts_with("error") => { ui.label(egui::RichText::new(state_label(&s)).color(ERR)); }
                _ => {}
            }
            ui.label(egui::RichText::new(format!("Os plug-ins são procurados em: {}", crate::ladspa::search_dirs().iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", "))).small().color(FORE2));
        }
        "stereo_widen" => {
            slider_row(ui, "Frequência de divisão", p, "crossover", 20.0..=2000.0, 500.0, " Hz");
            ui.label(egui::RichText::new("Dá a um som mono a sensação de estéreo. Um som que já é estéreo passa sem mudança.").small().color(FORE2));
        }
        "voice_denoise" => {
            let mut mono = p.num("mono", 0.0) > 0.5;
            if ui.checkbox(&mut mono, "Processar em mono").changed() { p.set("mono", if mono { 1.0 } else { 0.0 }); }
            ui.label(egui::RichText::new("Em mono, a entrada estéreo é juntada num canal só, o que poupa processador.").small().color(FORE2));
            let vad = rec.as_ref().map(|r| r.seconds as f32).unwrap_or(0.0);
            ui.add(egui::ProgressBar::new(vad.clamp(0.0, 1.0)).desired_width(240.0).text(if vad > 0.5 { "Voz presente" } else { "Sem voz" }));
            ui.label(egui::RichText::new("Tira o ruído de fundo da fala (RNNoise).").small().color(FORE2));
        }
        "convolution" => {
            slider_row(ui, "Ganho de entrada", p, "pregain", -24.0..=12.0, 0.0, " dB");
            ui.label(egui::RichText::new("Resposta a impulso (um arquivo WAV)").color(FORE2));
            let mut path = p.text("ir", "");
            if ui.add(egui::TextEdit::singleline(&mut path).hint_text("~/impulsos/sala.wav").desired_width(270.0)).changed() { p.set_text("ir", &path); }
            match rec.as_ref().map(|r| r.state.clone()) {
                Some(s) if s.starts_with("error") => { ui.label(egui::RichText::new(state_label(&s)).color(ERR)); }
                Some(s) if !s.is_empty() => { ui.label(egui::RichText::new(state_label(&s)).color(FORE1)); }
                _ => { ui.label(egui::RichText::new("Escolha um arquivo de áudio como resposta a impulso. Ele é carregado quando a sessão está rodando.").small().color(FORE2)); }
            }
            ui.label(egui::RichText::new("O impulso é convertido para a taxa da sessão (48 kHz) quando ela for diferente.").small().color(FORE2));
        }
        "delay" => {
            let mut d = p.num("delay", 0.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Atraso").color(FORE2));
                if ui.add(egui::Slider::new(&mut d, 0.0..=1000.0).suffix(" ms").integer()).changed() { p.set("delay", d.floor()); }
            });
            ui.label(egui::RichText::new("Em milissegundos inteiros, até 1000 ms: o bastante para alinhar o som com o vídeo de uma chamada ou transmissão.").small().color(FORE2));
        }
        "time_shift" => {
            let lag = rec.as_ref().map(|r| r.seconds).unwrap_or(0.0);
            ui.label(egui::RichText::new(if lag < 0.05 { "Ao vivo".to_string() } else { format!("{} s atrás do ao vivo", format!("{lag:.1}").replace('.', ",")) }).strong().color(if lag < 0.05 { GREEN } else { WARN }));
            let jump = |p: &mut Params, s: f64| { let n = p.num("jump_seq", 0.0); p.set("jump_s", s); p.set("jump_live", 0.0); p.set("jump_seq", n + 1.0); };
            ui.horizontal(|ui| {
                if ui.button("⏪ 30s").clicked() { jump(p, 30.0); }
                if ui.button("◀ 10s").clicked() { jump(p, 10.0); }
                let paused = p.num("paused", 0.0) > 0.5;
                if ui.button(if paused { "▶ Tocar" } else { "⏸ Pausar" }).clicked() { p.set("paused", if paused { 0.0 } else { 1.0 }); }
                if ui.button("10s ▶").clicked() { jump(p, -10.0); }
                if ui.button("30s ⏩").clicked() { jump(p, -30.0); }
            });
            if ui.button("Voltar ao vivo").clicked() { let n = p.num("live_seq", 0.0); p.set("jump_live", 1.0); p.set("live_seq", n + 1.0); }
            ui.label(egui::RichText::new("Guarda os últimos dois minutos para você pausar e voltar o áudio ao vivo.").small().color(FORE2));
        }
        "source_switch" => {
            let sw = p.num("switch", 0.0) as usize;
            for i in 0..2 {
                let name = ups.get(i).map(|x| x.1.clone()).unwrap_or_else(|| format!("Entrada {}", i + 1));
                if ui.selectable_label(sw == i, format!("Entrada {}: {}", i + 1, name)).clicked() { p.set("switch", i as f64); }
            }
        }
        "parametric_eq" => {
            let n = (p.num("bands", 3.0) as usize).clamp(0, 16);
            let types = ["Pico", "Prateleira grave", "Prateleira aguda", "Passa-baixa", "Passa-alta", "Passa-banda", "Rejeita-banda"];
            for i in 0..n {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        let mut on = p.num(&format!("b{i}_on"), 1.0) > 0.5;
                        if ui.checkbox(&mut on, format!("Banda {}", i + 1)).changed() { p.set(&format!("b{i}_on"), if on { 1.0 } else { 0.0 }); }
                        let t = (p.num(&format!("b{i}_type"), 0.0) as usize).min(types.len() - 1);
                        egui::ComboBox::from_id_salt(format!("pt{i}")).selected_text(types[t]).show_ui(ui, |ui| {
                            for (k, nm) in types.iter().enumerate() { if ui.selectable_label(t == k, *nm).clicked() { p.set(&format!("b{i}_type"), k as f64); } }
                        });
                    });
                    let mut f = p.num(&format!("b{i}_freq"), 1000.0);
                    ui.horizontal(|ui| { ui.label("Freq."); if ui.add(egui::Slider::new(&mut f, 20.0..=20000.0).logarithmic(true).suffix(" Hz")).changed() { p.set(&format!("b{i}_freq"), f); } });
                    slider_row(ui, "Ganho", p, &format!("b{i}_gain"), -24.0..=24.0, 0.0, " dB");
                    slider_row(ui, "Q", p, &format!("b{i}_q"), 0.1..=10.0, 1.0, "");
                });
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(n < 16, egui::Button::new("+ Banda")).clicked() {
                    let i = n;
                    p.set("bands", (n + 1) as f64);
                    p.set(&format!("b{i}_type"), 0.0); p.set(&format!("b{i}_freq"), 1000.0); p.set(&format!("b{i}_gain"), 0.0); p.set(&format!("b{i}_q"), 1.0); p.set(&format!("b{i}_on"), 1.0);
                }
                if ui.add_enabled(n > 0, egui::Button::new("− Banda")).clicked() { p.set("bands", (n - 1) as f64); }
            });
        }
        "high_pass" => slider_row(ui, "Corte", p, "freq", 20.0..=2000.0, 80.0, " Hz"),
        "band_pass" => {
            slider_row(ui, "Centro", p, "freq", 50.0..=15000.0, 1000.0, " Hz");
            slider_row(ui, "Q", p, "q", 0.1..=10.0, 1.0, "");
        }
        "high_shelf" | "low_shelf" => {
            slider_row(ui, "Frequência", p, "freq", 50.0..=18000.0, if kind == "high_shelf" { 6000.0 } else { 150.0 }, " Hz");
            slider_row(ui, "Ganho", p, "gain", -24.0..=24.0, 0.0, " dB");
        }
        "radio_stream" => {
            let text = |ui: &mut egui::Ui, label: &str, key: &str, p: &mut Params, pw: bool| {
                ui.label(egui::RichText::new(label).color(FORE2));
                let mut v = p.text(key, "");
                if ui.add(egui::TextEdit::singleline(&mut v).password(pw).desired_width(270.0)).changed() { p.set_text(key, &v); }
            };
            ui.label(egui::RichText::new("Tipo de servidor").color(FORE2));
            let cur = p.text("server", "Icecast 2");
            egui::ComboBox::from_id_salt("srv").selected_text(cur.clone()).show_ui(ui, |ui| {
                for s in ["Icecast 2", "Shoutcast 1", "Shoutcast 2"] { if ui.selectable_label(cur == s, s).clicked() { p.set_text("server", s); } }
            });
            if cur != "Icecast 2" { ui.label(egui::RichText::new("Usa a conexão de fonte clássica do Shoutcast, na porta + 1.").small().color(FORE2)); }
            text(ui, "Endereço do servidor", "host", p, false);
            slider_row(ui, "Porta", p, "port", 1.0..=65535.0, 8000.0, "");
            text(ui, "Usuário", "user", p, false);
            text(ui, "Senha", "password", p, true);
            if cur == "Icecast 2" { text(ui, "Ponto de montagem", "mount", p, false); } else if cur == "Shoutcast 2" { text(ui, "ID do fluxo (opcional)", "stream_id", p, false); }
            text(ui, "Nome da estação", "name", p, false);
            text(ui, "Gênero", "genre", p, false);
            text(ui, "Site", "url", p, false);
            let mut pubs = p.num("public", 0.0) > 0.5;
            if ui.checkbox(&mut pubs, "Aparecer no diretório público").changed() { p.set("public", if pubs { 1.0 } else { 0.0 }); }
            let fmt = p.text("format", "MP3");
            ui.horizontal(|ui| { ui.label(egui::RichText::new("Format").color(FORE2)); for f in ["MP3", "AAC"] { if ui.selectable_label(fmt == f, f).clicked() { p.set_text("format", f); } } });
            let br = p.num("bitrate", 128.0) as i32;
            ui.horizontal(|ui| { for (v, t) in [(32, "Fala · 32 kbps"), (128, "Média · 128 kbps"), (256, "Alta · 256 kbps")] { if ui.selectable_label(br == v, t).clicked() { p.set("bitrate", v as f64); } } });
            if let Some(r) = &rec { ui.label(egui::RichText::new(format!("{} · {}", state_label(&r.state), fmt_time(r.seconds))).color(if r.state.starts_with("error") { ERR } else { GREEN })); }
        }
        "rtmp_stream" => {
            ui.label(egui::RichText::new("Endereço do servidor").color(FORE2));
            let mut url = p.text("url", "");
            if ui.add(egui::TextEdit::singleline(&mut url).hint_text("rtmp://live.example.com/app").desired_width(270.0)).changed() { p.set_text("url", &url); }
            ui.label(egui::RichText::new("Chave da transmissão").color(FORE2));
            let mut key = p.text("key", "");
            if ui.add(egui::TextEdit::singleline(&mut key).password(true).desired_width(270.0)).changed() { p.set_text("key", &key); }
            let q = p.num("quality", 1.0) as i32;
            ui.horizontal(|ui| {
                for (v, t) in [(0, "Leve"), (1, "Padrão"), (2, "Alta")] {
                    let (vb, _) = engine::rtmp_quality(v);
                    if ui.selectable_label(q == v, format!("{t} · {} Mbps", vb / 1000)).clicked() { p.set("quality", v as f64); }
                }
            });
            ui.label(egui::RichText::new("Envia o seu áudio com uma imagem preta parada (via ffmpeg).").small().color(FORE2));
            if let Some(r) = &rec { ui.label(egui::RichText::new(format!("{} · {}", state_label(&r.state), fmt_time(r.seconds))).color(if r.state.starts_with("error") { ERR } else { GREEN })); }
        }
        "transcribe" => {
            let text = |ui: &mut egui::Ui, label: &str, key: &str, p: &mut Params, hint: &str| {
                ui.label(egui::RichText::new(label).color(FORE2));
                let mut v = p.text(key, "");
                if ui.add(egui::TextEdit::singleline(&mut v).hint_text(hint).desired_width(270.0)).changed() { p.set_text(key, &v); }
            };
            text(ui, "Arquivo do modelo (whisper.cpp, .bin)", "model", p, "~/modelos/ggml-base.bin");
            text(ui, "Idioma (auto, pt, en…)", "language", p, "auto");
            text(ui, "Programa", "command", p, "whisper-cli");
            text(ui, "Pasta", "folder", p, &engine::default_transcripts_dir());
            text(ui, "Nome do arquivo", "file_name", p, engine::DEFAULT_TRANSCRIPT_NAME);
            ui.label(egui::RichText::new("A cada 10 segundos o áudio vira texto e é acrescentado a um arquivo .txt. Precisa do whisper.cpp (whisper-cli) instalado.").small().color(FORE2));
            if let Some(r) = &rec { ui.label(egui::RichText::new(format!("{} · {}", state_label(&r.state), fmt_time(r.seconds))).color(if r.state.starts_with("error") { ERR } else { GREEN })); }
        }
        "recorder" => recorder_settings(ui, p, rec),
        _ => {}
    }
}

fn recorder_settings(ui: &mut egui::Ui, p: &mut Params, rec: Option<engine::RecStatus>) {
    ui.label(egui::RichText::new("Nome do arquivo").color(FORE2));
    let mut name = p.text("file_name", engine::DEFAULT_FILE_NAME);
    if ui.add(egui::TextEdit::singleline(&mut name).desired_width(270.0)).changed() { p.set_text("file_name", &name); }
    ui.horizontal_wrapped(|ui| {
        for (t, tok) in [("Sessão", "%name"), ("Contador", "%counter"), ("Data", "%date"), ("Hora", "%time"), ("Ano", "%year"), ("Mês", "%month"), ("Dia", "%day"), ("Horas", "%hour"), ("Minutos", "%minute"), ("Segundos", "%second"), ("Título", "%tag_title"), ("Artista", "%tag_artist"), ("Álbum", "%tag_album")] {
            if ui.small_button(t).clicked() { let n = p.text("file_name", ""); p.set_text("file_name", &format!("{n} {tok}")); }
        }
    });
    ui.label(egui::RichText::new("Pasta").color(FORE2));
    let mut folder = p.text("folder", &engine::default_recordings_dir());
    if ui.add(egui::TextEdit::singleline(&mut folder).desired_width(270.0)).changed() { p.set_text("folder", &folder); }

    ui.label(egui::RichText::new("Formato").color(FORE2));
    let ff = engine::ffmpeg_available();
    let cur = p.text("format", "WAV");
    egui::ComboBox::from_id_salt("fmt").width(230.0).selected_text(cur.clone()).show_ui(ui, |ui| {
        for f in ["WAV", "AIFF", "MP3", "AAC", "ALAC", "FLAC"] {
            let needs = !matches!(f, "WAV" | "AIFF");
            let label = if needs && !ff { format!("{f}  (precisa do ffmpeg)") } else { f.to_string() };
            if ui.selectable_label(cur == f, label).clicked() { p.set_text("format", f); }
        }
    });
    match cur.as_str() {
        "WAV" => {
            let d = p.num("depth", 16.0) as i32;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Resolução").color(FORE2));
                for (v, t) in [(16, "16 bits"), (24, "24 bits"), (32, "32 bits (ponto flutuante)")] { if ui.selectable_label(d == v, t).clicked() { p.set("depth", v as f64); } }
            });
            if d == 16 {
                let dm = p.num("dither", 0.0) as i32;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Dither").color(FORE2));
                    for (v, t) in [(0, "Nenhum"), (1, "TPDF plano"), (2, "Com modelagem de ruído")] { if ui.selectable_label(dm == v, t).clicked() { p.set("dither", v as f64); } }
                });
            }
        }
        "MP3" | "AAC" => {
            let br = p.num("bitrate", 256.0) as i32;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Qualidade").color(FORE2));
                for (v, t) in [(32, "Fala · 32 kbps"), (128, "Média · 128 kbps"), (256, "Alta · 256 kbps")] { if ui.selectable_label(br == v, t).clicked() { p.set("bitrate", v as f64); } }
            });
        }
        _ => {}
    }
    if matches!(cur.as_str(), "MP3" | "AAC" | "ALAC" | "FLAC") && !ff {
        ui.label(egui::RichText::new("O ffmpeg não está instalado, então o arquivo será salvo em WAV.").small().color(WARN));
    }

    egui::CollapsingHeader::new("Mais opções de qualidade").default_open(false).show(ui, |ui| {
        let rate = p.num("q_rate", 0.0) as u32;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Taxa de amostragem").color(FORE2));
            egui::ComboBox::from_id_salt("qrate").selected_text(if rate == 0 { "A mesma da sessão".to_string() } else { format!("{rate} Hz") }).show_ui(ui, |ui| {
                if ui.selectable_label(rate == 0, "A mesma da sessão").clicked() { p.set("q_rate", 0.0); }
                for r in [22050u32, 32000, 44100, 48000, 88200, 96000] { if ui.selectable_label(rate == r, format!("{r} Hz")).clicked() { p.set("q_rate", r as f64); } }
            });
        });
        let mono = p.num("q_channels", 0.0) as i32 == 1;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Canais").color(FORE2));
            if ui.selectable_label(!mono, "Estéreo").clicked() { p.set("q_channels", 0.0); }
            if ui.selectable_label(mono, "Mono").clicked() { p.set("q_channels", 1.0); }
        });
        if matches!(cur.as_str(), "AAC" | "ALAC") {
            let mp4 = p.num("q_mp4", 0.0) > 0.5;
            ui.horizontal(|ui| {
                if ui.selectable_label(!mp4, ".m4a (abre em mais lugares)").on_hover_text("O arquivo só fica pronto quando a gravação termina").clicked() { p.set("q_mp4", 0.0); }
                if ui.selectable_label(mp4, ".mp4 (aguenta interrupções)").on_hover_text("Gravado em pedaços: se o programa fechar de repente, o que já foi gravado continua tocando").clicked() { p.set("q_mp4", 1.0); }
            });
        }
        if cur == "MP3" {
            let mut vbr = p.num("q_vbr", 0.0) > 0.5;
            if ui.checkbox(&mut vbr, "Taxa de bits variável").changed() { p.set("q_vbr", if vbr { 1.0 } else { 0.0 }); }
        }
        if matches!(cur.as_str(), "ALAC" | "FLAC" | "AIFF") {
            let d = p.num("depth", 16.0) as i32;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Bits por amostra").color(FORE2));
                for (v, t) in [(16, "16 bits"), (24, "24 bits")] { if ui.selectable_label(d == v, t).clicked() { p.set("depth", v as f64); } }
            });
        }
    });
    egui::CollapsingHeader::new("Etiquetas").default_open(false).show(ui, |ui| {
        let mut row = |ui: &mut egui::Ui, label: &str, key: &str, w: f32| {
            ui.horizontal(|ui| {
                ui.add_sized([86.0, 18.0], egui::Label::new(egui::RichText::new(label).color(FORE2)));
                let mut v = p.text(key, "");
                if ui.add(egui::TextEdit::singleline(&mut v).desired_width(w)).changed() { p.set_text(key, &v); }
            });
        };
        row(ui, "Título", "tag_title", 190.0);
        row(ui, "Artista", "tag_artist", 190.0);
        row(ui, "Álbum", "tag_album", 190.0);
        row(ui, "Gênero", "tag_genre", 190.0);
        row(ui, "Ano", "tag_year", 60.0);
        row(ui, "Faixa", "tag_track", 40.0);
        row(ui, "Total de faixas", "tag_trackcount", 40.0);
        row(ui, "Disco", "tag_disc", 40.0);
        row(ui, "Total de discos", "tag_disccount", 40.0);
        row(ui, "Comentário", "tag_comment", 190.0);
        row(ui, "Capa (imagem)", "tag_art", 190.0);
        let mut comp = p.num("tag_compilation", 0.0) > 0.5;
        if ui.checkbox(&mut comp, "Coletânea").changed() { p.set("tag_compilation", if comp { 1.0 } else { 0.0 }); }
        ui.label(egui::RichText::new("As etiquetas vão para arquivos MP3, AAC, ALAC, FLAC, WAV e AIFF. Também podem entrar no nome do arquivo pelos botões Título, Artista e Álbum.").small().color(FORE2));
    });
    ui.add_space(4.0);
    ui.label(egui::RichText::new("LIMITES DO ARQUIVO").small().color(FORE2));
    limit_row(ui, p, "split", "Começar outro arquivo a cada", &[("megabytes", "space*1"), ("gigabytes", "space*1024"), ("minutos", "time*60"), ("horas", "time*3600")]);
    limit_row(ui, p, "stop", "Terminar a gravação depois de", &[("minutos", "time*60"), ("horas", "time*3600"), ("megabytes", "space*1"), ("gigabytes", "space*1024")]);
    let mut on = p.num("silence_on", 0.0) > 0.5;
    if ui.checkbox(&mut on, "Detectar silêncio").changed() { p.set("silence_on", if on { 1.0 } else { 0.0 }); }
    if on {
        let a = p.num("silence_action", 1.0) as i32;
        ui.horizontal(|ui| {
            for (v, t) in [(1, "Pular o silêncio"), (2, "Começar outro arquivo"), (3, "Terminar a gravação")] { if ui.selectable_label(a == v, t).clicked() { p.set("silence_action", v as f64); } }
        });
        slider_row(ui, "depois de", p, "silence_time", 0.5..=99.9, engine::SILENCE_SECONDS, " s");
        slider_row(ui, "abaixo de", p, "silence_db", -90.0..=-10.0, engine::SILENCE_DB, " dB");
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let paused = p.num("pause", 0.0) > 0.5;
        if ui.button(if paused { "Continuar" } else { "Pausar" }).clicked() { p.set("pause", if paused { 0.0 } else { 1.0 }); }
        if ui.button("Dividir arquivo").on_hover_text("Fecha o arquivo atual e continua num novo").clicked() { let n = p.num("split_req", 0.0); p.set("split_req", n + 1.0); }
    });
    if let Some(r) = rec {
        ui.label(egui::RichText::new(format!("{} · {} · {}", state_label(&r.state), fmt_time(r.seconds), fmt_size(r.bytes))).color(if r.state.starts_with("error") { ERR } else { FORE1 }));
        if !r.file.is_empty() { ui.label(egui::RichText::new(&r.file).small().color(FORE2)); }
    }
}

fn limit_row(ui: &mut egui::Ui, p: &mut Params, key: &str, label: &str, units: &[(&str, &str)]) {
    let mut on = p.num(&format!("{key}_on"), 0.0) > 0.5;
    ui.horizontal(|ui| {
        if ui.checkbox(&mut on, label).changed() { p.set(&format!("{key}_on"), if on { 1.0 } else { 0.0 }); }
    });
    if on {
        ui.horizontal(|ui| {
            let mut v = p.num(&format!("{key}_size"), 1.0);
            if ui.add(egui::DragValue::new(&mut v).range(0.1..=10000.0).speed(0.1)).changed() { p.set(&format!("{key}_size"), v); }
            let cur = p.text(&format!("{key}_unit"), units[0].1);
            let lab = units.iter().find(|u| u.1 == cur).map(|u| u.0).unwrap_or(units[0].0);
            egui::ComboBox::from_id_salt(format!("{key}_u")).selected_text(lab).show_ui(ui, |ui| {
                for (t, k) in units { if ui.selectable_label(cur == *k, *t).clicked() { p.set_text(&format!("{key}_unit"), k); } }
            });
        });
    }
}

#[allow(dead_code)]
fn _keep(_: Arc<()>) {}

/// Headless check of the engine: a test tone through Volume and a 10-band EQ into a WAV recorder.
pub fn selftest(dir: &str) {
    let mut s = Session::new("Self Test");
    let app = std::env::var("KALANGO_AUDIO_SELFTEST_APP").ok();
    let a = s.add_block(if app.is_some() { "app_source" } else { "test_tone" }, 2.0, 2.0);
    let v = s.add_block("volume", 8.0, 2.0);
    let e = s.add_block("graphic_eq", 14.0, 2.0);
    let r = s.add_block("recorder", 20.0, 2.0);
    s.connect(a, v); s.connect(v, e); s.connect(e, r);
    for b in s.blocks.iter_mut() {
        if b.id == v { b.p.set("gain", 1.0); b.p.set("boost", 1.0); }
        if b.id == a { b.p.set("level", 0.9); if let Some(n) = &app { b.p.set_text("app", n); } }
        if b.id == r { b.p.set_text("folder", dir); b.p.set_text("file_name", "selftest %name %counter"); b.p.set_text("format", "WAV"); }
    }
    let eng = Engine::start(s.blocks.clone(), &s.name);
    std::thread::sleep(std::time::Duration::from_millis(if app.is_some() { 5000 } else { 2500 }));
    let st = eng.shared.rec.lock().unwrap().clone();
    for (k, v) in st { println!("recorder {k}: {} {:.2}s {} bytes {}", v.state, v.seconds, v.bytes, v.file); }
    drop(eng);
}

/// Headless checks of the newer blocks; each scenario records two seconds into its own folder.
pub fn selftest_all(dir: &str) {
    fn scenario(dir: &str, name: &str, build: impl Fn(&mut Session) -> u64, secs: u64) {
        let mut s = Session::new(name);
        let last = build(&mut s);
        let r = s.add_block("recorder", 40.0, 2.0);
        s.connect(last, r);
        for b in s.blocks.iter_mut() {
            if b.id == r { b.p.set_text("folder", &format!("{dir}/{name}")); b.p.set_text("file_name", "out"); b.p.set_text("format", "WAV"); }
        }
        let eng = Engine::start(s.blocks.clone(), &s.name);
        std::thread::sleep(std::time::Duration::from_millis(secs * 1000));
        let st = eng.shared.rec.lock().unwrap().clone();
        for (k, v) in st { println!("{name}: block {k}: {} {:.2}s", v.state, v.seconds); }
        drop(eng);
    }
    let tone = |s: &mut Session, f: f64, l: f64| { let t = s.add_block("test_tone", 2.0, 2.0); for b in s.blocks.iter_mut() { if b.id == t { b.p.set("freq", f); b.p.set("level", l); } } t };
    scenario(dir, "mixer", |s| {
        let a = tone(s, 440.0, 0.2); let b = tone(s, 880.0, 0.2);
        let m = s.add_block("mixer", 20.0, 2.0);
        s.connect(a, m); s.connect(b, m);
        for x in s.blocks.iter_mut() { if x.id == m { x.p.set("m1", 1.0); } }
        m
    }, 2);
    scenario(dir, "ducking", |s| {
        let ov = tone(s, 1000.0, 0.1); let bg = tone(s, 200.0, 0.5);
        let d = s.add_block("ducking", 20.0, 2.0);
        s.connect(bg, d); s.connect(ov, d);
        for x in s.blocks.iter_mut() { if x.id == d { x.p.set("overlay", ov as f64); } }
        d
    }, 2);
    scenario(dir, "sync", |s| {
        let a = tone(s, 440.0, 0.5);
        let y = s.add_block("delay", 20.0, 2.0);
        s.connect(a, y);
        for x in s.blocks.iter_mut() { if x.id == y { x.p.set("delay", 300.0); } }
        y
    }, 2);
    scenario(dir, "paramEQ", |s| {
        let a = tone(s, 1000.0, 0.3);
        let y = s.add_block("parametric_eq", 20.0, 2.0);
        s.connect(a, y);
        for x in s.blocks.iter_mut() { if x.id == y { x.p.set("b1_gain", 12.0); } }
        y
    }, 2);
    scenario(dir, "denoise", |s| {
        let a = tone(s, 1000.0, 0.3);
        let y = s.add_block("noise_reduction", 20.0, 2.0);
        s.connect(a, y);
        y
    }, 2);
    scenario(dir, "denoise2", |s| {
        let a = tone(s, 1000.0, 0.3);
        let y = s.add_block("noise_reduction", 20.0, 2.0);
        s.connect(a, y);
        for x in s.blocks.iter_mut() { if x.id == y { x.p.set("learn_seq", 1.0); } }
        y
    }, 5);
    if let Ok(url) = std::env::var("KALANGO_AUDIO_RTMP_URL") {
        scenario(dir, "rtmp", |s| {
            let a = tone(s, 440.0, 0.3);
            let y = s.add_block("rtmp_stream", 20.0, 2.0);
            s.connect(a, y);
            for x in s.blocks.iter_mut() { if x.id == y { x.p.set_text("url", &url); x.p.set_text("key", "k"); x.p.set("quality", 0.0); } }
            y
        }, 6);
    }
    scenario(dir, "broadcast", |s| {
        let a = tone(s, 440.0, 0.3);
        let y = s.add_block("radio_stream", 20.0, 2.0);
        s.connect(a, y);
        y
    }, 1);
    scenario(dir, "transcribe", |s| {
        let a = tone(s, 440.0, 0.3);
        let y = s.add_block("transcribe", 20.0, 2.0);
        s.connect(a, y);
        y
    }, 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_falls_at_twenty_db_per_second() {
        let mut d = Disp::default();
        d.update(&Meter { peak: [1.0, 0.1], rms: [0.5, 0.0] }, 1);
        assert_eq!(d.peak[0], 1.0);
        let start_rms = d.rms[0];
        // one second of silence (60 ticks) takes 20 dB = 1/3 of the 60 dB scale off the bars
        d.update(&Meter::default(), 60);
        assert!((d.peak[0] - (1.0 - 1.0 / 3.0)).abs() < 1e-4, "peak {}", d.peak[0]);
        assert!((d.rms[0] - (start_rms - 1.0 / 3.0)).abs() < 1e-4, "rms {}", d.rms[0]);
        // a new louder reading jumps straight up
        d.update(&Meter { peak: [1.0, 0.0], rms: [0.0, 0.0] }, 1);
        assert_eq!(d.peak[0], 1.0);
        // and three seconds of silence empty it
        d.update(&Meter::default(), 180);
        assert_eq!(d.peak[0], 0.0);
    }

    #[test]
    fn hold_marker_stays_for_a_second_and_a_half_then_falls() {
        let mut d = Disp::default();
        d.update(&Meter { peak: [0.5, 0.0], rms: [0.2, 0.0] }, 1);
        let held = d.hold[0];
        assert!((held - (60.0 - 6.0206) / 60.0).abs() < 1e-3, "hold at {held}");
        let quiet = Meter::default();
        d.update(&quiet, 89);
        assert_eq!(d.hold[0], held, "hold stays put for 1.5 s");
        d.update(&quiet, 1 + 60);
        assert!((d.hold[0] - (held - 1.0 / 3.0)).abs() < 0.01, "then falls at 20 dB/s, at {}", d.hold[0]);
        // a higher peak takes over at once
        d.update(&Meter { peak: [1.0, 0.0], rms: [0.0, 0.0] }, 1);
        assert_eq!(d.hold[0], 1.0);
    }

    #[test]
    fn clip_lights_at_full_scale_and_holds_two_seconds() {
        let mut d = Disp::default();
        d.update(&Meter { peak: [1.0, 0.999], rms: [0.5, 0.1] }, 1);
        assert!(d.clip[0] > 0.3 && d.clip[1] == 0.0);
        d.update(&Meter::default(), 119);
        assert!(d.clip[0] > 0.3, "still lit just under 2 s");
        d.update(&Meter::default(), 2);
        assert_eq!(d.clip[0], 0.0);
    }

    #[test]
    fn vu_level_integrates_in_300_ms() {
        let mut d = Disp::default();
        let tone = Meter { peak: [0.14, 0.14], rms: [0.1, 0.1] };
        d.update(&tone, 6); // 100 ms: well under the final value
        assert!(d.vu_lin[0] < 0.08, "{}", d.vu_lin[0]);
        d.update(&tone, 12); // 300 ms in total: 99 %
        assert!((d.vu_lin[0] - 0.1).abs() < 0.0015, "{}", d.vu_lin[0]);
        assert!((d.vu[0] - 40.0 / 60.0).abs() < 0.005);
    }

    #[test]
    fn file_name_tokens() {
        let s = engine::expand_tokens("%name %counter %prefixed_counter %tag_title", "Show", 7);
        assert!(s.starts_with("Show 7 007"));
        let d = engine::expand_tokens("%date %time", "x", 1);
        assert_eq!(d.len(), 19); // 2026-11-18 22.53.10
        assert!(!engine::expand_tokens("a/b:c", "x", 1).contains('/'));
    }

    #[test]
    fn templates_load_and_wire_up() {
        let t = templates();
        assert_eq!(t.len(), 12);
        for tpl in &t {
            let s = tpl.instantiate();
            assert_eq!(s.blocks.len(), tpl.blocks.len());
            for b in &s.blocks { assert!(kind_info(&b.kind).is_some(), "unknown block kind {} in {}", b.kind, tpl.name); }
            let wires: usize = s.blocks.iter().map(|b| b.outs.len()).sum();
            let expected: usize = tpl.blocks.iter().map(|b| b.out.len()).sum();
            assert_eq!(wires, expected, "{}", tpl.name);
        }
    }

    #[test]
    fn sessions_survive_saving_and_loading() {
        let s = templates()[3].instantiate();
        let text = serde_json::to_string(&vec![s.clone()]).unwrap();
        let back: Vec<Session> = serde_json::from_str(&text).unwrap();
        assert_eq!(back[0], s);
    }

    #[test]
    fn connecting_rules() {
        let mut s = Session::new("t");
        let a = s.add_block("test_tone", 0.0, 0.0);
        let b = s.add_block("volume", 5.0, 0.0);
        s.connect(a, b);
        s.connect(b, a); // a source cannot take input
        s.connect(a, a);
        assert_eq!(s.block(a).unwrap().outs, vec![b]);
        assert!(s.block(b).unwrap().outs.is_empty());
        s.remove_block(b);
        assert!(s.block(a).unwrap().outs.is_empty());
    }
}

/// Headless checks of the recorder's file rules (split, stop, silence, formats).
pub fn selftest_rec(dir: &str) {
    fn go(dir: &str, name: &str, level: f64, secs: u64, set: &dyn Fn(&mut Params)) {
        let mut s = Session::new(name);
        let t = s.add_block("test_tone", 2.0, 2.0);
        let r = s.add_block("recorder", 10.0, 2.0);
        s.connect(t, r);
        for b in s.blocks.iter_mut() {
            if b.id == t { b.p.set("level", level); }
            if b.id == r { b.p.set_text("folder", &format!("{dir}/{name}")); b.p.set_text("file_name", "take %counter"); b.p.set_text("format", "WAV"); set(&mut b.p); }
        }
        let eng = Engine::start(s.blocks.clone(), name);
        std::thread::sleep(std::time::Duration::from_millis(secs * 1000));
        for (_, v) in eng.shared.rec.lock().unwrap().iter() { println!("{name}: state={}", v.state); }
        drop(eng);
    }
    go(dir, "split", 0.3, 5, &|p| { p.set("split_on", 1.0); p.set("split_size", 0.03); p.set_text("split_unit", "time*60"); });
    go(dir, "stop", 0.3, 3, &|p| { p.set("stop_on", 1.0); p.set("stop_size", 0.02); p.set_text("stop_unit", "time*60"); });
    go(dir, "silence_remove", 0.0, 4, &|p| { p.set("silence_on", 1.0); p.set("silence_action", 1.0); p.set("silence_time", 1.0); });
    go(dir, "silence_end", 0.0, 4, &|p| { p.set("silence_on", 1.0); p.set("silence_action", 3.0); p.set("silence_time", 1.0); });
    go(dir, "aiff", 0.3, 2, &|p| { p.set_text("format", "AIFF"); });
    go(dir, "mp3", 0.3, 2, &|p| { p.set_text("format", "MP3"); p.set("bitrate", 128.0); });
    go(dir, "wav24", 0.3, 2, &|p| { p.set("depth", 24.0); });
    go(dir, "paused", 0.3, 2, &|p| { p.set("pause", 1.0); });
}

/// Runs a heavy chain for a few seconds and reports how busy the audio thread was.
pub fn selftest_load() {
    let mut s = Session::new("Load");
    let a = s.add_block("test_tone", 2.0, 2.0);
    let chain = ["graphic_eq", "compressor", "noise_reduction", "voice_denoise", "loudness", "stereo_widen", "parametric_eq", "tone_control", "hum_removal"];
    let mut prev = a;
    for k in chain { let b = s.add_block(k, 8.0, 2.0); s.connect(prev, b); prev = b; }
    let eng = Engine::start(s.blocks.clone(), "Load");
    std::thread::sleep(std::time::Duration::from_millis(4000));
    let l = eng.shared.load.load(std::sync::atomic::Ordering::Relaxed);
    let o = eng.shared.overruns.load(std::sync::atomic::Ordering::Relaxed);
    println!("audio load {:.1}% , chunks that ran late: {}", l as f32 / 10.0, o);
}


/// The Application block's settings: which program (any installed one), opening an address, and the extra options.
fn app_source_settings(ui: &mut egui::Ui, p: &mut Params, apps_now: &[audio::AppInfo]) {
    ui.label(egui::RichText::new("Programa").color(FORE2));
    let cur = p.text("app", "Web Browser");
    let mut names: Vec<(String, String)> = vec![];
    for a in apps_now { let key = format!("name:{}", a.name.to_lowercase()); if !names.iter().any(|n| n.0 == key) { names.push((key, a.name.clone())); } }
    let search_id = ui.make_persistent_id("appsearch");
    egui::ComboBox::from_id_salt("app").width(270.0).selected_text(audio::app_label(&cur)).height(420.0).show_ui(ui, |ui| {
        let mut filter: String = ui.data_mut(|d| d.get_temp::<String>(search_id)).unwrap_or_default();
        ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Procurar programas instalados").desired_width(250.0));
        ui.data_mut(|d| d.insert_temp(search_id, filter.clone()));
        let f = filter.trim().to_lowercase();
        if f.is_empty() {
            for key in ["Web Browser", "Call App", "Any Application"] {
                if ui.selectable_label(cur == key, audio::app_label(key)).clicked() { p.set_text("app", key); }
            }
            ui.separator();
            ui.label(egui::RichText::new("Tocando som agora").small().color(FORE2));
            if names.is_empty() { ui.label(egui::RichText::new("(nada tocando)").small().color(FORE2)); }
            for (key, label) in &names { if ui.selectable_label(&cur == key, label).clicked() { p.set_text("app", key); } }
            ui.separator();
            ui.label(egui::RichText::new("Programas instalados").small().color(FORE2));
        }
        let mut shown = 0;
        for d in audio::installed_apps() {
            if !f.is_empty() && !d.name.to_lowercase().contains(&f) && !d.bin.to_lowercase().contains(&f) { continue; }
            let key = format!("desktop:{}", d.id);
            if ui.selectable_label(cur == key, &d.name).clicked() { p.set_text("app", &key); }
            shown += 1;
            if shown >= 400 { break; }
        }
        if shown == 0 { ui.label(egui::RichText::new("Nenhum programa instalado com esse nome").small().color(FORE2)); }
    });
    let matching: Vec<&str> = apps_now.iter().filter(|a| audio::app_matches(&cur, a)).map(|a| a.name.as_str()).collect();
    if matching.is_empty() {
        ui.label(egui::RichText::new("Esse programa ainda não está tocando nada. Quando ele começar, o som é captado em um ou dois segundos.").small().color(FORE2));
    } else {
        ui.label(egui::RichText::new(format!("Captando: {}", matching.join(", "))).small().color(GREEN));
    }
    let mut open_url = p.num("url_on", 0.0) > 0.5;
    if ui.checkbox(&mut open_url, "Abrir um endereço").on_hover_text("Abre este endereço (no programa escolhido) quando a sessão começa").changed() { p.set("url_on", if open_url { 1.0 } else { 0.0 }); }
    if open_url {
        let mut url = p.text("url", "");
        if ui.add(egui::TextEdit::singleline(&mut url).hint_text("https://…").desired_width(270.0)).changed() { p.set_text("url", &url); }
    }
    ui.label(egui::RichText::new("Capta só o som deste programa (pelo PipeWire), em qualquer saída que ele use, mesmo com outros programas tocando.").small().color(FORE2));
    egui::CollapsingHeader::new("Mais opções").default_open(false).show(ui, |ui| {
        let mut inputs = p.num("capture_inputs", 0.0) > 0.5;
        if ui.checkbox(&mut inputs, "Juntar o microfone").changed() { p.set("capture_inputs", if inputs { 1.0 } else { 0.0 }); }
        let mut split = p.num("split_channels", 0.0) > 0.5;
        ui.add_enabled_ui(inputs, |ui| { if ui.checkbox(&mut split, "Separar nos canais").changed() { p.set("split_channels", if split { 1.0 } else { 0.0 }); } });
        ui.label(egui::RichText::new("Com os canais separados, o microfone fica na esquerda e o som do programa na direita.").small().color(FORE2));
        ui.label(egui::RichText::new("Quando o programa para de tocar, o bloco segue mandando silêncio, para a gravação não ter buracos.").small().color(FORE2));
    });
}
