//! Small building blocks shared by the screens, in the KalangoOS look (classes in assets/css/base.css).

use super::{icons, theme};
use crate::meter::Disp;
use crate::model::{kind_info, Cat};
use gtk::prelude::*;
use gtk::{glib, Align, Orientation};
use kalango_movimento as mov;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub fn bx(o: Orientation, spacing: i32, class: &str) -> gtk::Box {
    let b = gtk::Box::new(o, spacing);
    for c in class.split_whitespace() {
        b.add_css_class(c);
    }
    b
}

pub fn hbox(spacing: i32, class: &str) -> gtk::Box {
    bx(Orientation::Horizontal, spacing, class)
}

pub fn vbox(spacing: i32, class: &str) -> gtk::Box {
    bx(Orientation::Vertical, spacing, class)
}

/// A left-aligned label (class names separated by spaces).
pub fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in class.split_whitespace() {
        l.add_css_class(c);
    }
    l
}

/// A label that wraps inside its column.
pub fn para(text: &str, class: &str) -> gtk::Label {
    let l = label(text, class);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_max_width_chars(1);
    l.set_hexpand(true);
    l
}

/// A label cut with "…" when there is no room.
pub fn line(text: &str, class: &str) -> gtk::Label {
    let l = label(text, class);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(1);
    l.set_hexpand(true);
    l
}

/// Section caption ("SUAS SESSÕES").
pub fn caps(text: &str) -> gtk::Label {
    label(&text.to_uppercase(), "ka-caps")
}

pub fn spacer() -> gtk::Box {
    let b = gtk::Box::new(Orientation::Horizontal, 0);
    b.set_hexpand(true);
    b
}

/// Kinds of button: "primario" (accent), "secundario", "fantasma" (no fill), "gravar" (red), "perigo".
pub fn button(text: &str, kind: &str, icon: Option<&str>) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("ka-botao");
    b.add_css_class(&format!("ka-{kind}"));
    let row = hbox(6, "");
    row.set_halign(Align::Center);
    let fg = match kind {
        "primario" | "gravar" | "perigo" => "accent_fg",
        "fantasma" => "text2",
        _ => "text",
    };
    if let Some(i) = icon {
        row.append(&icons::icon(i, fg, 14));
    }
    if !text.is_empty() {
        row.append(&gtk::Label::new(Some(text)));
    } else {
        b.add_css_class("ka-so-icone");
    }
    b.set_child(Some(&row));
    b.set_valign(Align::Center);
    b
}

/// A square glyph button (menus, close, "more").
pub fn icon_button(icon: &str, token: &str, px: i32, tip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("ka-icone");
    b.set_child(Some(&icons::icon(icon, token, px)));
    if !tip.is_empty() {
        b.set_tooltip_text(Some(tip));
    }
    b.set_valign(Align::Center);
    b
}

/// The colour class of a token: chips, tiles and dots use `cor-<name>`.
pub fn color_class(token: &str) -> String {
    format!("cor-{}", token.trim_start_matches("cor/").trim_start_matches("k_"))
}

/// A small tinted tag ("reunião", "aprovar").
pub fn chip(text: &str, token: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("ka-chip");
    l.add_css_class(&color_class(token));
    l.set_valign(Align::Center);
    l
}

/// A dot of state (8 px circle).
pub fn dot(token: &str) -> gtk::Box {
    let d = hbox(0, "ka-ponto");
    d.add_css_class(&color_class(token));
    d.set_valign(Align::Center);
    d
}

/// The token of each category of block (Design System: source teal, effect accent, output gold, advanced blue, meter purple).
pub fn cat_token(c: Cat) -> &'static str {
    match c {
        Cat::Source => "teal",
        Cat::Effect => "accent",
        Cat::Output => "gold",
        Cat::Advanced => "blue",
        Cat::Meter => "purple",
    }
}

pub fn kind_token(kind: &str) -> &'static str {
    kind_info(kind).map(|k| cat_token(k.cat)).unwrap_or("muted")
}

/// The glyph of each kind of block.
pub fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "app_source" => "apps-pontos",
        "device_input" | "voice_denoise" | "voice_isolate" => "microfone",
        "system_sound" => "volume",
        "test_tone" => "anel",
        "device_output" => "fone",
        "recorder" => "download",
        "rtmp_stream" => "video-d",
        "radio_stream" => "compartilhar",
        "transcribe" => "doc",
        "level_meter" | "needle_meter" => "colunas",
        "ducking" => "cerebro",
        "mixer" => "lista",
        "source_switch" => "ramo",
        "time_shift" => "relogio",
        "delay" => "relogio",
        "ladspa_plugin" => "camadas",
        "convolution" => "agrupar",
        "noise_reduction" | "click_removal" | "hum_removal" => "escudo",
        _ => "ajustes",
    }
}

/// An icon in a tinted square of its colour (`size` px; the glyph is 45 % of it).
pub fn tile(icon: &str, token: &str, size: i32) -> gtk::Box {
    let t = hbox(0, "ka-tile");
    t.add_css_class(&color_class(token));
    t.set_size_request(size, size);
    t.set_valign(Align::Center);
    t.set_halign(Align::Start);
    let i = icons::icon(icon, token, (size as f32 * 0.5).round() as i32);
    i.set_hexpand(true);
    t.append(&i);
    t
}

/// The tile of a block kind.
pub fn kind_tile(kind: &str, size: i32) -> gtk::Box {
    tile(kind_icon(kind), kind_token(kind), size)
}

/// A session's blocks in miniature, in signal order: sources → … → outputs, joined by short lines.
pub fn chain(kinds: &[String], size: i32, gap: i32) -> gtk::Box {
    let row = hbox(0, "ka-cadeia");
    row.set_halign(Align::Start);
    for (i, k) in kinds.iter().enumerate() {
        if i > 0 {
            let l = hbox(0, "ka-cadeia-liga");
            l.set_size_request(gap, 1);
            l.set_valign(Align::Center);
            row.append(&l);
        }
        let t = kind_tile(k, size);
        t.set_tooltip_text(kind_info(k).map(|x| crate::i18n::tr(x.title)));
        row.append(&t);
    }
    row
}

/// The kinds of a session in signal order (sources first, outputs last), at most `max`.
pub fn ordered_kinds(s: &crate::model::Session, max: usize) -> Vec<String> {
    let rank = |k: &str| match kind_info(k).map(|x| x.cat) {
        Some(Cat::Source) => 0,
        Some(Cat::Output) => 3,
        Some(Cat::Meter) => 2,
        _ => 1,
    };
    let mut v: Vec<(i32, f32, String)> = s.blocks.iter().map(|b| (rank(&b.kind), b.x, b.kind.clone())).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)));
    v.into_iter().map(|x| x.2).take(max).collect()
}

pub fn switch(on: bool) -> gtk::Switch {
    let s = gtk::Switch::new();
    s.set_active(on);
    s.set_valign(Align::Center);
    s
}

/// An entry with a glyph on the left.
pub fn entry(placeholder: &str, icon: Option<&str>) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.add_css_class("ka-campo");
    e.set_placeholder_text(Some(placeholder));
    if let Some(i) = icon {
        e.set_primary_icon_paintable(icons::texture(i, "muted", 14).as_ref());
    }
    e
}

/// A drop-down of texts.
pub fn dropdown(items: &[String], selected: usize) -> gtk::DropDown {
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let d = gtk::DropDown::from_strings(&refs);
    d.add_css_class("ka-seletor");
    d.set_selected(selected.min(items.len().saturating_sub(1)) as u32);
    d.set_valign(Align::Center);
    d
}

/// A labelled slider row: "Label ............ value" over a scale.
pub fn slider(title: &str, min: f64, max: f64, step: f64, value: f64, fmt: impl Fn(f64) -> String + 'static) -> (gtk::Box, gtk::Scale) {
    let b = vbox(6, "ka-controle");
    let top = hbox(0, "");
    top.append(&line(title, "ka-texto2"));
    let v = label(&fmt(value), "ka-valor");
    top.append(&v);
    b.append(&top);
    let s = gtk::Scale::with_range(Orientation::Horizontal, min, max, step);
    s.set_value(value);
    s.set_draw_value(false);
    s.set_hexpand(true);
    s.connect_value_changed(move |s| v.set_text(&fmt(s.value())));
    b.append(&s);
    (b, s)
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

pub fn scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_hscrollbar_policy(gtk::PolicyType::Never);
    s.set_child(Some(child));
    s.set_vexpand(true);
    s.set_hexpand(true);
    s
}

/// Wraps a widget so it can spring in (see [`appear`]).
pub fn animated(w: &impl IsA<gtk::Widget>) -> mov::Animavel {
    mov::Animavel::new(w)
}

/// A list of things comes in as a cascade (24 ms apart, rápida spring), as the Design System's lists do.
pub fn cascade(items: &[mov::Animavel]) {
    mov::cascata(items, 12.0);
}

/// A panel or page comes in: fades and slides `dy` px with the suave spring.
pub fn appear(a: &mov::Animavel, dx: f64, dy: f64) {
    a.entrar((dx, dy), 0.98);
}

// ---------------- drawings that follow the theme and the meters ----------------

thread_local! {
    static DRAWINGS: RefCell<Vec<glib::WeakRef<gtk::DrawingArea>>> = const { RefCell::new(Vec::new()) };
    static HOOKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Redrawn when the theme changes.
pub fn follow_theme(d: &gtk::DrawingArea) {
    if !HOOKED.with(|h| h.replace(true)) {
        theme::on_change(|| {
            DRAWINGS.with(|l| {
                l.borrow_mut().retain(|w| w.upgrade().map(|d| d.queue_draw()).is_some());
            })
        });
    }
    DRAWINGS.with(|l| {
        let mut l = l.borrow_mut();
        if l.len() % 128 == 127 {
            l.retain(|w| w.upgrade().is_some());
        }
        l.push(d.downgrade());
    });
}

/// A rounded rectangle path.
pub fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

/// One meter bar: well, teal body, gold above −15 dB (0.75), the held peak as a tick, red when it clipped.
pub fn draw_bar(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, level: f32, hold: f32, clip: bool) {
    theme::source(cr, "sunken", 1.0);
    rounded(cr, x, y, w, h, h.min(4.0) / 2.0);
    let _ = cr.fill();
    let lv = (level.clamp(0.0, 1.0) as f64) * w;
    if lv > 0.5 {
        theme::source(cr, "teal", 1.0);
        cr.rectangle(x, y, lv.min(w * 0.75), h);
        let _ = cr.fill();
        if lv > w * 0.75 {
            theme::source(cr, "gold", 1.0);
            cr.rectangle(x + w * 0.75, y, lv - w * 0.75, h);
            let _ = cr.fill();
        }
    }
    if hold > 0.01 {
        theme::source(cr, if clip { "red" } else { "text" }, 1.0);
        cr.rectangle(x + (hold.clamp(0.0, 1.0) as f64 * w - 2.0).max(0.0), y, 2.0, h);
        let _ = cr.fill();
    }
}

/// Two bars (left over right) filling the area; `None` draws empty wells.
pub fn draw_stereo(cr: &gtk::cairo::Context, w: f64, h: f64, d: Option<&Disp>) {
    let gap = if h >= 10.0 { 2.0 } else { 1.0 };
    let bh = ((h - gap) / 2.0).max(1.0);
    for c in 0..2 {
        let (lv, hold, clip) = d.map(|d| (d.peak[c], d.hold[c], d.clip[c] > 0.0)).unwrap_or((0.0, 0.0, false));
        draw_bar(cr, 0.0, c as f64 * (bh + gap), w, bh, lv, hold, clip);
    }
}

/// A live stereo meter: `read` gives the current reading; redraw it on every tick.
pub fn meter(width: i32, height: i32, read: impl Fn() -> Option<Disp> + 'static) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(width);
    a.set_content_height(height);
    a.set_valign(Align::Center);
    a.add_css_class("ka-medidor");
    a.set_draw_func(move |_, cr, w, h| draw_stereo(cr, w as f64, h as f64, read().as_ref()));
    follow_theme(&a);
    a
}

// ---------------- waveforms ----------------

thread_local! {
    static PEAKS: RefCell<HashMap<PathBuf, Rc<Vec<f32>>>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<HashMap<PathBuf, Vec<glib::WeakRef<gtk::DrawingArea>>>> = RefCell::new(HashMap::new());
}

/// The loudness of a file in `n` slices (0..1), read in a helper thread: WAV directly, other formats through ffmpeg.
pub fn read_peaks(path: &Path, n: usize) -> Vec<f32> {
    let mut samples: Vec<f32> = Vec::new();
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
    if ext == "wav" {
        if let Ok(mut r) = hound::WavReader::open(path) {
            let spec = r.spec();
            let step = (spec.channels as usize).max(1) * 8;
            match spec.sample_format {
                hound::SampleFormat::Float => samples = r.samples::<f32>().step_by(step).filter_map(Result::ok).collect(),
                hound::SampleFormat::Int => {
                    let max = (1i64 << (spec.bits_per_sample.max(1) - 1)) as f32;
                    samples = r.samples::<i32>().step_by(step).filter_map(Result::ok).map(|s| s as f32 / max).collect()
                }
            }
        }
    } else if crate::media::have("ffmpeg") {
        if let Ok(out) = std::process::Command::new("ffmpeg")
            .args(["-v", "quiet", "-i"])
            .arg(path)
            .args(["-ac", "1", "-ar", "4000", "-f", "s16le", "-"])
            .output()
        {
            samples = out.stdout.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
        }
    }
    if samples.is_empty() {
        return vec![0.0; n];
    }
    let per = (samples.len() / n).max(1);
    let mut v: Vec<f32> = samples.chunks(per).take(n).map(|c| c.iter().fold(0f32, |m, s| m.max(s.abs()))).collect();
    let top = v.iter().cloned().fold(0.0001f32, f32::max);
    for x in v.iter_mut() {
        *x = (*x / top).sqrt();
    }
    v.resize(n, 0.0);
    v
}

/// The peaks of a file once they are read (None while reading; the area is redrawn when they arrive).
pub fn peaks_for(path: &Path, area: &gtk::DrawingArea) -> Option<Rc<Vec<f32>>> {
    if let Some(p) = PEAKS.with(|c| c.borrow().get(path).cloned()) {
        return Some(p);
    }
    let first = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let e = p.entry(path.to_path_buf()).or_default();
        e.push(area.downgrade());
        e.len() == 1
    });
    if first {
        let path = path.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<f32>>();
        let p2 = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(read_peaks(&p2, 240));
        });
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || match rx.try_recv() {
            Ok(v) => {
                PEAKS.with(|c| c.borrow_mut().insert(path.clone(), Rc::new(v)));
                for w in PENDING.with(|p| p.borrow_mut().remove(&path)).unwrap_or_default() {
                    if let Some(a) = w.upgrade() {
                        a.queue_draw();
                    }
                }
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => glib::ControlFlow::Break,
        });
    }
    None
}

/// Draws bars of a waveform: the played part in `played_token`, the rest in `rest_token`, and the playhead.
pub fn draw_wave(cr: &gtk::cairo::Context, w: f64, h: f64, peaks: &[f32], played: Option<f64>, played_token: &str, rest_token: &str) {
    if peaks.is_empty() {
        return;
    }
    let (bw, gap) = (2.0, 1.5);
    let n = ((w + gap) / (bw + gap)).floor().max(1.0) as usize;
    for i in 0..n {
        let p = peaks[(i * peaks.len() / n).min(peaks.len() - 1)] as f64;
        let bh = (p * h).max(1.5);
        let x = i as f64 * (bw + gap);
        let is_played = played.map(|f| x < f * w).unwrap_or(false);
        theme::source(cr, if is_played { played_token } else { rest_token }, 1.0);
        cr.rectangle(x, (h - bh) / 2.0, bw, bh);
        let _ = cr.fill();
    }
    if let Some(f) = played {
        theme::source(cr, "text", 1.0);
        cr.rectangle((f * w - 1.0).max(0.0), 0.0, 2.0, h);
        let _ = cr.fill();
    }
}

/// A waveform preview of a file. `played` gives the fraction played (None: just the shape).
pub fn wave(path: &Path, width: i32, height: i32, token: &str, played: impl Fn() -> Option<f64> + 'static) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_content_width(width);
    a.set_content_height(height);
    a.add_css_class("ka-onda");
    let (path, token) = (path.to_path_buf(), token.to_string());
    a.set_draw_func(move |a, cr, w, h| {
        if let Some(p) = peaks_for(&path, a) {
            let pl = played();
            draw_wave(cr, w as f64, h as f64, &p, pl, if pl.is_some() { "accent" } else { &token }, if pl.is_some() { "dim" } else { &token });
        }
    });
    follow_theme(&a);
    a
}
