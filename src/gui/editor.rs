//! The session editor (designs "Editor de sessão (gravando)", "Editor · arrastar bloco da biblioteca", the right half
//! of "Estados" and the narrow window of "Janela estreita"): the session bar (back, name, start/stop with the running
//! time, the agent, the schedule and more), the block library on the left, the canvas in the middle and the
//! inspector of the selected block on the right. Under 1280 px the inspector becomes a bottom sheet; under 900 px
//! the library hides behind a "+" button.
//!
//! For captures: `KALANGO_AUDIO_SESSAO=<name>` opens that session instead of the first one,
//! `KALANGO_AUDIO_SESSAO_RODAR=1` also starts it, and `KALANGO_AUDIO_BLOCO=<kind>` selects its first block of that
//! kind (e.g. `compressor`).

use super::editor_canvas::{self as canvas_mod, Canvas};
use super::editor_inspector::Inspector;
use super::editor_library::Library;
use super::widgets::*;
use super::{icons, Change, Layout, Ui};
use crate::core::{fmt_size, fmt_time, Core};
use crate::i18n::{tr, trf};
use crate::model::{kind_info, Block};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, Align};
use kalango_movimento as mov;
use mov::Mola;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ---------------------------------------------------------------- changes to blocks (shared by the parts)

/// Runs a discrete edit of the open session as one undo step (Ctrl+Z restores it at once).
pub fn edit<R>(ui: &Rc<Ui>, f: impl FnOnce(&mut Core) -> R) -> R {
    let mut c = ui.core.borrow_mut();
    c.checkpoint();
    let r = f(&mut c);
    c.checkpoint();
    r
}

/// Changes a block of the open session. Returns false when it is gone.
pub fn with_block(ui: &Rc<Ui>, id: u64, f: impl FnOnce(&mut Block)) -> bool {
    let mut c = ui.core.borrow_mut();
    match c.session_mut().and_then(|s| s.blocks.iter_mut().find(|b| b.id == id)) {
        Some(b) => {
            f(b);
            true
        }
        None => false,
    }
}

/// On/off (off = bypass: the sound passes unchanged).
pub fn toggle_block(ui: &Rc<Ui>, id: u64) {
    let found = edit(ui, |c| match c.session_mut().and_then(|s| s.blocks.iter_mut().find(|b| b.id == id)) {
        Some(b) => {
            b.enabled = !b.enabled;
            true
        }
        None => false,
    });
    if found {
        ui.changed(Change::Sessions);
    }
}

/// A copy of the block with the same settings, below it; it becomes the selection.
pub fn duplicate_block(ui: &Rc<Ui>, id: u64) {
    let new = {
        let mut c = ui.core.borrow_mut();
        c.checkpoint();
        let Some(s) = c.session_mut() else { return };
        let Some(src) = s.block(id).cloned() else { return };
        let (mut x, mut y) = (src.x, src.y + 4.0);
        while s.blocks.iter().any(|b| (b.x - x).abs() < 5.0 && (b.y - y).abs() < 3.0) {
            y += 4.0;
            if y > src.y + 40.0 {
                x += 6.0;
                y = src.y;
            }
        }
        let nid = s.add_block(&src.kind, x, y);
        if let Some(nb) = s.blocks.iter_mut().find(|b| b.id == nid) {
            nb.p = src.p.clone();
            nb.enabled = src.enabled;
        }
        c.sel = Some(nid);
        c.checkpoint();
        nid
    };
    let _ = new;
    ui.changed(Change::Sessions);
    ui.changed(Change::Selection);
}

/// Removes a block (in automatic wiring the blocks around it are joined).
pub fn remove_block(ui: &Rc<Ui>, id: u64) {
    edit(ui, |c| c.remove_block_smart(id));
    ui.changed(Change::Sessions);
    ui.changed(Change::Selection);
}

/// Switches between automatic and manual wiring. Existing wires are never changed by the switch: turning automatic
/// wiring back on offers to connect the blocks left without wires, left to right, and does it only when asked.
pub fn toggle_manual(ui: &Rc<Ui>) {
    let (manual, plan) = edit(ui, |c| {
        let Some(s) = c.session_mut() else { return (false, vec![]) };
        s.manual = !s.manual;
        let plan = if s.manual { vec![] } else { canvas_mod::rewire_plan(s) };
        (s.manual, plan)
    });
    ui.changed(Change::Sessions);
    if manual {
        ui.toast(tr("Manual wiring: connect the blocks yourself"), None);
    } else if plan.is_empty() {
        ui.toast(tr("Automatic wiring: new blocks join their neighbours"), None);
    } else {
        ui.toast(
            tr("Automatic wiring is on. Some blocks have no wires yet."),
            Some((
                tr("Connect them"),
                Box::new(|u: &Rc<Ui>| {
                    edit(u, |c| {
                        if let Some(s) = c.session_mut() {
                            // worked out again: the session may have changed since the offer
                            for (a, b) in canvas_mod::rewire_plan(s) {
                                s.connect(a, b);
                            }
                        }
                    });
                    u.changed(Change::Sessions);
                }),
            )),
        );
    }
}

/// Shows the messages Core left for the person (a start it refused, and why) as toasts.
fn show_notices(ui: &Rc<Ui>) {
    let notes: Vec<String> = std::mem::take(&mut ui.core.borrow_mut().notices);
    for n in notes {
        if n == tr("This would play the computer’s sound again on top of itself") {
            ui.toast(&format!("{n}. {}", tr("Use headphones on another device, or remove the output.")), None);
        } else {
            ui.toast(&n, None);
        }
    }
}

// ---------------------------------------------------------------- what a block says (pure)

fn pct(v: f64) -> String {
    format!("{} %", (v * 100.0).round() as i64)
}

fn hz(f: f64) -> String {
    if f >= 1000.0 {
        format!("{} kHz", format!("{:.1}", f / 1000.0).trim_end_matches(".0").replace('.', ","))
    } else {
        format!("{} Hz", f.round() as i64)
    }
}

fn db(v: f64) -> String {
    let s = format!("{:+.0} dB", v);
    s.replace('-', "−")
}

/// The name of a device for a person ("default" is the system's).
pub fn device_name(id: &str, devices: &[(String, String)]) -> String {
    if id.is_empty() || id == "default" {
        return tr("System default").to_string();
    }
    devices.iter().find(|d| d.0 == id).map(|d| d.1.clone()).unwrap_or_else(|| id.to_string())
}

pub const CHANNEL_MODES: [&str; 7] = [
    crate::i18n::n_("Normal"),
    crate::i18n::n_("Swap left and right"),
    crate::i18n::n_("Left on both sides"),
    crate::i18n::n_("Right on both sides"),
    crate::i18n::n_("Join into mono"),
    crate::i18n::n_("Mute the left"),
    crate::i18n::n_("Mute the right"),
];

fn side(v: f64) -> String {
    if v.abs() < 0.01 {
        tr("Centre").to_string()
    } else if v < 0.0 {
        trf("Left {n}", &[("n", ((-v * 100.0).round() as i64).to_string())])
    } else {
        trf("Right {n}", &[("n", ((v * 100.0).round() as i64).to_string())])
    }
}

/// The one-line summary of a block's main setting, shown under its name ("Speech · 75 %", "WAV · 24 bits"…).
pub fn summary(b: &Block, devices: &[(String, String)]) -> String {
    let p = &b.p;
    match b.kind.as_str() {
        "volume" => {
            let mut s = pct(crate::dsp::gain_total(p) as f64);
            if p.num("muted", 0.0) > 0.5 {
                s = tr("Muted").to_string();
            }
            s
        }
        "device_input" | "system_sound" => device_name(&p.text("device", "default"), devices),
        "device_output" => {
            let d = device_name(&p.text("device", "default"), devices);
            if p.num("muted", 0.0) > 0.5 {
                format!("{d} · {}", tr("Muted"))
            } else {
                format!("{d} · {}", pct(p.num("gain", 1.0)))
            }
        }
        "app_source" => crate::audio::app_label(&p.text("app", "Web Browser")),
        "test_tone" => format!("{} · {}", hz(p.num("freq", 440.0)), pct(p.num("level", 0.25))),
        "graphic_eq" => {
            let bands: Vec<f64> = (0..10).map(|i| p.num(&format!("band{i}"), 0.5)).collect();
            crate::presets::eq_presets()
                .iter()
                .find(|(_, v)| v.iter().zip(&bands).all(|(a, b)| (a - b).abs() < 0.005))
                .map(|(n, _)| tr(n).to_string())
                .unwrap_or_else(|| tr("Custom").to_string())
        }
        "compressor" => {
            let mode = crate::presets::compressor_mode_of(p).map(tr).unwrap_or(tr("Custom"));
            format!("{mode} · {}", pct(p.num("amount", 1.0)))
        }
        "loudness" => trf("Strength {n}", &[("n", pct(p.num("amount", 0.0)))]),
        "recorder" => {
            let f = p.text("format", "WAV");
            match f.as_str() {
                "WAV" => {
                    let d = p.num("depth", 16.0) as i64;
                    if d == 32 {
                        format!("WAV · {}", tr("32-bit float"))
                    } else {
                        trf("{format} · {n} bits", &[("format", f.clone()), ("n", d.to_string())])
                    }
                }
                "MP3" | "AAC" => format!("{f} · {} kbps", p.num("bitrate", 256.0) as i64),
                _ => f,
            }
        }
        "transcribe" => {
            let lang = p.text("language", "auto");
            let lang = if lang == "auto" || lang.is_empty() { tr("any language").to_string() } else { lang };
            trf("Live · {lang}", &[("lang", lang)])
        }
        "voice_isolate" => trf("Intensity {amount}", &[("amount", pct(p.num("amount", 1.0)))]),
        "voice_denoise" => if p.num("mono", 0.0) > 0.5 { tr("Mono · RNNoise") } else { tr("Stereo · RNNoise") }.to_string(),
        "noise_reduction" => trf("Noise {db}", &[("db", db(-p.num("reduction", 6.0)))]),
        "balance" => side(p.num("balance", 0.0)),
        "pan" => side(p.num("pan", 0.0)),
        "tone_control" => trf(
            "Bass {bass} · treble {treble}",
            &[("bass", db((p.num("bass", 0.5) - 0.5) * 24.0)), ("treble", db((p.num("treble", 0.5) - 0.5) * 24.0))],
        ),
        "low_pass" => hz(p.num("freq", 10000.0)),
        "stereo_widen" => trf("Split at {f}", &[("f", hz(p.num("crossover", 500.0)))]),
        "high_pass" => hz(p.num("freq", 80.0)),
        "band_pass" => format!("{} · Q {}", hz(p.num("freq", 1000.0)), format!("{:.1}", p.num("q", 1.0)).replace('.', ",")),
        "high_shelf" => format!("{} · {}", hz(p.num("freq", 6000.0)), db(p.num("gain", 0.0))),
        "low_shelf" => format!("{} · {}", hz(p.num("freq", 150.0)), db(p.num("gain", 0.0))),
        "channels" => tr(CHANNEL_MODES[(p.num("mode", 0.0).max(0.0) as usize).min(CHANNEL_MODES.len() - 1)]).to_string(),
        "hum_removal" => format!("{} × {}", hz(p.num("base", 50.0)), p.num("amount", 3.0).round() as i64),
        "click_removal" => trf("Sensitivity {n}", &[("n", (p.num("sens", 3.0).round() as i64).to_string())]),
        "mixer" => trf("{n} tracks", &[("n", (0..5).filter(|i| p.num(&format!("m{i}"), 0.0) < 0.5).count().to_string())]),
        "ducking" => format!("{} · {}", db(p.num("threshold", -30.0)), pct(p.num("ducked", 0.4))),
        "delay" => format!("{} ms", p.num("delay", 0.0).floor() as i64),
        "time_shift" => if p.num("paused", 0.0) > 0.5 { tr("Paused") } else { tr("Live") }.to_string(),
        "source_switch" => trf("Input {n}", &[("n", (p.num("switch", 0.0) as i64 + 1).to_string())]),
        "parametric_eq" => trf("{n} bands", &[("n", (p.num("bands", 3.0) as i64).to_string())]),
        "radio_stream" => {
            let host = p.text("host", "");
            let fmt = format!("{} · {} kbps", p.text("format", "MP3"), p.num("bitrate", 128.0) as i64);
            if host.is_empty() {
                format!("{} · {fmt}", p.text("server", "Icecast 2"))
            } else {
                format!("{host} · {fmt}")
            }
        }
        "rtmp_stream" => {
            let url = p.text("url", "");
            let host = url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("").to_string();
            if host.is_empty() {
                tr("No server yet").to_string()
            } else {
                host
            }
        }
        "level_meter" => tr("Peak and average").to_string(),
        "needle_meter" => tr("VU needle").to_string(),
        "convolution" => {
            let ir = p.text("ir", "");
            let n = ir.rsplit('/').next().unwrap_or("").to_string();
            if n.is_empty() {
                tr("No impulse yet").to_string()
            } else {
                n
            }
        }
        "ladspa_plugin" => {
            let k = p.text("plugin", "");
            if k.is_empty() {
                tr("Choose a plug-in").to_string()
            } else {
                crate::ladspa::find_cached(&k).map(|x| x.name).unwrap_or_else(|| tr("Plug-in not found").to_string())
            }
        }
        _ => String::new(),
    }
}

/// The second line of a card: (show the red recording dot, text). Live state while the session runs.
pub fn status_line(b: &Block, running: bool, rec: Option<&crate::engine::RecStatus>, c: &Core) -> (bool, String) {
    let p = &b.p;
    match b.kind.as_str() {
        "recorder" => match rec {
            Some(st) if running && !st.state.is_empty() => (st.state == "recording", canvas_mod::recorder_line(st)),
            _ => {
                let folder = p.text("folder", "");
                let name = folder.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
                (false, if name.is_empty() { String::new() } else { trf("Folder {name}", &[("name", name)]) })
            }
        },
        "transcribe" => match rec {
            Some(st) if running && !st.state.is_empty() => (false, format!("{} · {}", crate::core::state_label(&st.state), fmt_time(st.seconds))),
            _ => match crate::voz::status() {
                crate::voz::Status::Ready => (false, tr("Local voice model · text as it hears").to_string()),
                _ => (true, tr("Voice model not installed").to_string()),
            },
        },
        "radio_stream" | "rtmp_stream" => match rec {
            Some(st) if running && !st.state.is_empty() => (false, format!("{} · {}", crate::core::state_label(&st.state), fmt_time(st.seconds))),
            _ => (false, String::new()),
        },
        "time_shift" if running => {
            let lag = rec.map(|r| r.seconds).unwrap_or(0.0);
            (false, if lag < 0.05 { tr("Live").to_string() } else { trf("{s} s behind", &[("s", format!("{lag:.1}").replace('.', ","))]) })
        }
        "compressor" | "loudness" => (false, tr("Reduction").to_string()),
        _ => {
            let _ = c;
            (false, String::new())
        }
    }
}

/// The blocks that feed `id` (their ids and names), in wiring order.
pub fn upstream(c: &Core, id: u64) -> Vec<(u64, String)> {
    let Some(s) = c.session() else { return vec![] };
    let mut v: Vec<(u64, String)> =
        s.blocks.iter().filter(|b| b.outs.contains(&id)).map(|b| (b.id, kind_info(&b.kind).map(|k| tr(k.title).to_string()).unwrap_or_default())).collect();
    v.sort_by_key(|x| x.0);
    v
}

/// The loudest input of a block right now (Disp of its upstream blocks).
pub fn input_disp(c: &Core, id: u64) -> Option<crate::meter::Disp> {
    let s = c.session()?;
    let uid = s.uid;
    s.blocks.iter().filter(|b| b.outs.contains(&id)).filter_map(|b| c.disp.get(&(uid, b.id))).copied().max_by(|a, b| a.rms_max().total_cmp(&b.rms_max()))
}

/// A meter reading (0..1 on the 60 dB scale) in dBFS.
pub fn level_db(v: f32) -> f32 {
    v * 60.0 - 60.0
}

/// The compressor's threshold and ratio for a block's settings.
pub fn comp_settings(b: &Block) -> crate::dsp::CompSettings {
    let def = if b.kind == "loudness" { 0.0 } else { 1.0 };
    crate::dsp::CompSettings::from_amount(b.p.num("amount", def) as f32, b.p.num("speed", 0.0) as f32, b.p.num("makeup", 1.0) as f32)
}

/// How much a compressor is turning the sound down now (dB, ≤ 0), from its input level and its curve.
pub fn gain_reduction(b: &Block, input_db: f32) -> f32 {
    let s = comp_settings(b);
    crate::dsp::Compressor::curve(input_db, s.threshold_db, s.ratio).min(0.0)
}

pub fn gain_reduction_of(c: &Core, id: u64) -> Option<f32> {
    let b = c.session()?.block(id)?;
    if !c.is_running(c.session()?.uid) || !b.enabled {
        return Some(0.0);
    }
    let d = input_disp(c, id)?;
    Some(gain_reduction(b, level_db(d.rms_max())))
}

// ---------------------------------------------------------------- the bar

struct Bar {
    root: gtk::CenterBox,
    caption: gtk::Label,
    name: gtk::Label,
    name_entry: gtk::Entry,
    name_stack: gtk::Stack,
    start: gtk::Button,
    stop: gtk::Button,
    timer: gtk::Label,
    dot: gtk::Box,
    state: gtk::Label,
    detail: gtk::Label,
    texts: gtk::Box,
    right: gtk::Box,
    manual: gtk::CheckButton,
    rate: gtk::DropDown,
    syncing: Cell<bool>,
}

const RATES: [u32; 4] = [44100, 48000, 88200, 96000];

fn build_bar(ui: &Rc<Ui>) -> Rc<Bar> {
    let root = gtk::CenterBox::new();
    root.add_css_class("ka-barra-sessao");
    // back + name
    let left = hbox(4, "");
    let back = icon_button("seta-esquerda", "text2", 16, tr("Back to the sessions"));
    {
        let u = ui.clone();
        back.connect_clicked(move |_| u.go("sessoes"));
    }
    left.append(&back);
    let names = vbox(0, "");
    names.set_valign(Align::Center);
    let caption = label(tr("Sessions"), "ka-fraco2 ka-sessao-rotulo");
    names.append(&caption);
    let name = label("", "ka-titulo2");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(28);
    name.set_tooltip_text(Some(tr("Double-click to rename")));
    let name_entry = gtk::Entry::new();
    name_entry.add_css_class("ka-campo");
    name_entry.add_css_class("ka-nome-sessao");
    let name_stack = gtk::Stack::new();
    name_stack.set_hhomogeneous(false);
    name_stack.add_named(&name, Some("rotulo"));
    name_stack.add_named(&name_entry, Some("campo"));
    names.append(&name_stack);
    left.append(&names);
    root.set_start_widget(Some(&left));

    // start/stop, time, state
    let mid = hbox(14, "");
    mid.set_valign(Align::Center);
    let start = button(tr("Start"), "primario", Some("tocar"));
    let stop = button(tr("Stop"), "gravar", Some("parar"));
    start.set_tooltip_text(Some(tr("Start the session (Space)")));
    stop.set_tooltip_text(Some(tr("Stop the session (Space)")));
    for b in [&start, &stop] {
        let u = ui.clone();
        b.connect_clicked(move |_| {
            let cur = u.core.borrow().cur;
            u.toggle(cur);
        });
        mid.append(b);
    }
    let timer = label("00:00:00", "ka-cronometro ka-tnum");
    mid.append(&timer);
    let texts = vbox(1, "");
    texts.set_valign(Align::Center);
    let st_row = hbox(6, "");
    let dot = dot("red");
    dot.add_css_class("ka-pulsa");
    st_row.append(&dot);
    let state = label("", "ka-estado-sessao");
    st_row.append(&state);
    texts.append(&st_row);
    let detail = label("", "ka-fraco2 ka-tnum");
    texts.append(&detail);
    mid.append(&texts);
    root.set_center_widget(Some(&mid));

    // agent, schedule, more
    let right = hbox(4, "");
    right.set_valign(Align::Center);
    let ask = button(tr("Ask the agent"), "fantasma", Some("spark"));
    {
        let u = ui.clone();
        ask.connect_clicked(move |_| super::agente::ask(&u, ""));
    }
    let sched = button(tr("Schedule session"), "fantasma", Some("relogio"));
    {
        let u = ui.clone();
        sched.connect_clicked(move |_| u.go("agenda"));
    }
    right.append(&ask);
    right.append(&sched);
    let more = gtk::MenuButton::new();
    more.add_css_class("ka-icone");
    more.set_child(Some(&icons::icon("pontos-v", "muted", 16)));
    more.set_tooltip_text(Some(tr("More")));
    let pop = gtk::Popover::new();
    pop.add_css_class("menu");
    let items = vbox(2, "");
    let manual = gtk::CheckButton::with_label(tr("Connect blocks by hand"));
    manual.add_css_class("ka-item-menu");
    let rate_names: Vec<String> = RATES.iter().map(|r| format!("{} Hz", r)).collect();
    let rate = dropdown(&rate_names, 1);
    {
        type Act = Box<dyn Fn(&Rc<Ui>, &gtk::Popover)>;
        let acts: Vec<(&str, Act)> = vec![
            (
                tr("Duplicate session"),
                Box::new(|u, _| {
                    u.core.borrow_mut().duplicate_session();
                    u.changed(Change::Sessions);
                    u.changed(Change::Selection);
                }),
            ),
            (tr("Export session…"), Box::new(|u, _| export_session(u))),
            (tr("Delete session"), Box::new(|u, _| delete_session(u))),
        ];
        for (text, f) in acts {
            let b = gtk::Button::with_label(text);
            b.add_css_class("ka-item-menu");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let (u, p) = (ui.clone(), pop.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                f(&u, &p);
            });
            items.append(&b);
        }
    }
    let sep = hbox(0, "ka-linha-div");
    sep.set_margin_top(4);
    sep.set_margin_bottom(4);
    items.append(&sep);
    items.append(&manual);
    let rr = hbox(8, "ka-item-menu-linha");
    let rl = label(tr("Sample rate"), "ka-texto2");
    rl.set_hexpand(true);
    rr.append(&rl);
    rr.append(&rate);
    items.append(&rr);
    pop.set_child(Some(&items));
    more.set_popover(Some(&pop));
    right.append(&more);
    root.set_end_widget(Some(&right));

    let bar = Rc::new(Bar {
        root,
        caption,
        name,
        name_entry,
        name_stack,
        start,
        stop,
        timer,
        dot,
        state,
        detail,
        texts,
        right,
        manual,
        rate,
        syncing: Cell::new(false),
    });
    {
        let (u, b) = (ui.clone(), bar.clone());
        bar.manual.connect_toggled(move |m| {
            if b.syncing.get() {
                return;
            }
            let want = m.is_active();
            let is = u.core.borrow().session().map(|s| s.manual).unwrap_or(false);
            if want != is {
                toggle_manual(&u);
            }
        });
        let (u, b) = (ui.clone(), bar.clone());
        bar.rate.connect_selected_notify(move |d| {
            if b.syncing.get() {
                return;
            }
            let r = RATES[(d.selected() as usize).min(RATES.len() - 1)];
            let running = {
                let mut c = u.core.borrow_mut();
                let uid = c.session().map(|s| s.uid).unwrap_or(0);
                if let Some(s) = c.session_mut() {
                    s.sample_rate = r;
                }
                c.is_running(uid)
            };
            if running {
                u.toast(tr("The new sample rate is used the next time the session starts"), None);
            }
            u.changed(Change::Sessions);
        });
    }
    // rename: double click on the name, Enter keeps it, Esc gives up
    {
        let click = gtk::GestureClick::new();
        let b = bar.clone();
        click.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                b.name_entry.set_text(&b.name.text());
                b.name_stack.set_visible_child_name("campo");
                b.name_entry.grab_focus();
            }
        });
        bar.name.add_controller(click);
        let (u, b) = (ui.clone(), bar.clone());
        let commit = Rc::new(move || {
            if b.name_stack.visible_child_name().as_deref() != Some("campo") {
                return;
            }
            b.name_stack.set_visible_child_name("rotulo");
            let text = b.name_entry.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            let changed = {
                let mut c = u.core.borrow_mut();
                match c.session_mut() {
                    Some(s) if s.name != text => {
                        s.name = text;
                        true
                    }
                    _ => false,
                }
            };
            if changed {
                u.changed(Change::Sessions);
            }
        });
        let c2 = commit.clone();
        bar.name_entry.connect_activate(move |_| c2());
        let focus = gtk::EventControllerFocus::new();
        let c3 = commit.clone();
        focus.connect_leave(move |_| c3());
        bar.name_entry.add_controller(focus);
        let keys = gtk::EventControllerKey::new();
        let b = bar.clone();
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Escape {
                b.name_stack.set_visible_child_name("rotulo");
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        bar.name_entry.add_controller(keys);
    }
    bar
}

impl Bar {
    fn sync(&self, ui: &Rc<Ui>) {
        let c = ui.core.borrow();
        let Some(s) = c.session() else { return };
        if self.name_stack.visible_child_name().as_deref() != Some("campo") {
            self.name.set_text(&s.name);
        }
        self.syncing.set(true);
        self.manual.set_active(s.manual);
        let r = if s.sample_rate == 0 { 48000 } else { s.sample_rate };
        self.rate.set_selected(RATES.iter().position(|x| *x == r).unwrap_or(1) as u32);
        self.syncing.set(false);
        drop(c);
        self.tick(ui);
    }

    /// The time, the state and the file size (twice a second is enough for the text; the dot pulses in CSS).
    fn tick(&self, ui: &Rc<Ui>) {
        let c = ui.core.borrow();
        let Some(s) = c.session() else { return };
        let uid = s.uid;
        let running = c.is_running(uid);
        let recording = c.is_recording(uid);
        self.start.set_visible(!running);
        self.stop.set_visible(running);
        let t = c.running_for(uid).unwrap_or(0.0);
        self.timer.set_text(&fmt_time(t));
        set_cls(&self.timer, "parado", !running);
        set_cls(&self.timer, "gravando", recording);
        let rate = if s.sample_rate == 0 { 48000 } else { s.sample_rate };
        let khz = format!("{} kHz", format!("{:.1}", rate as f64 / 1000.0).trim_end_matches(".0").replace('.', ","));
        if recording {
            // the recorder that is writing: its size and format
            let rec = s.blocks.iter().filter(|b| b.kind == "recorder").find_map(|b| c.rec_status(uid, b.id).filter(|r| r.state == "recording").map(|r| (b, r)));
            let detail = rec.map(|(b, r)| format!("{} · {} {khz}", fmt_size(r.bytes), b.p.text("format", "WAV"))).unwrap_or(khz);
            self.dot.set_visible(true);
            self.dot.remove_css_class("cor-teal");
            self.dot.add_css_class("cor-red");
            self.dot.add_css_class("ka-pulsa");
            self.state.set_text(tr("Recording now"));
            self.state.set_css_classes(&["ka-estado-sessao", "cor-red"]);
            self.detail.set_text(&detail);
            self.detail.set_visible(true);
        } else if running {
            self.dot.set_visible(true);
            self.dot.remove_css_class("cor-red");
            self.dot.remove_css_class("ka-pulsa");
            self.dot.add_css_class("cor-teal");
            self.state.set_text(tr("Running · not recording"));
            self.state.set_css_classes(&["ka-estado-sessao", "cor-teal"]);
            self.detail.set_text(&khz);
            self.detail.set_visible(true);
        } else {
            self.dot.set_visible(false);
            self.state.set_text(tr("Stopped · ready to record"));
            self.state.set_css_classes(&["ka-estado-sessao", "parado"]);
            self.detail.set_visible(false);
        }
    }

    fn set_layout(&self, l: Layout) {
        let narrow = l == Layout::Narrow;
        self.caption.set_visible(!narrow);
        self.texts.set_visible(!narrow);
        self.right.set_visible(!narrow);
        // narrow: name on the left, the time and Start/Stop on the right (as in "Janela estreita")
        let mid = self.timer.parent().and_downcast::<gtk::Box>();
        if let Some(mid) = mid {
            if narrow {
                mid.reorder_child_after(&self.timer, None::<&gtk::Widget>);
                if self.root.center_widget().is_some() {
                    self.root.set_center_widget(None::<&gtk::Widget>);
                    self.root.set_end_widget(Some(&mid));
                }
            } else if self.root.center_widget().is_none() {
                mid.reorder_child_after(&self.timer, Some(&self.stop));
                self.root.set_end_widget(None::<&gtk::Widget>);
                self.root.set_center_widget(Some(&mid));
                self.root.set_end_widget(Some(&self.right));
            }
        }
    }
}

fn set_cls(w: &impl IsA<gtk::Widget>, c: &str, on: bool) {
    if on {
        w.add_css_class(c);
    } else {
        w.remove_css_class(c);
    }
}

fn export_session(ui: &Rc<Ui>) {
    let name = ui.core.borrow().session().map(|s| s.name.clone()).unwrap_or_default();
    let d = gtk::FileDialog::new();
    d.set_title(tr("Export session"));
    d.set_initial_name(Some(&format!("{name}.json")));
    let u = ui.clone();
    d.save(Some(&ui.window), gio::Cancellable::NONE, move |r| {
        let Ok(f) = r else { return };
        let Some(p) = f.path() else { return };
        let res = u.core.borrow_mut().export_session(&p.to_string_lossy());
        match res {
            Ok(()) => u.toast(&trf("Session saved to “{path}”", &[("path", p.display().to_string())]), None),
            Err(e) => u.toast(&e, None),
        }
    });
}

fn delete_session(ui: &Rc<Ui>) {
    let (i, removed) = {
        let mut c = ui.core.borrow_mut();
        let i = c.cur;
        let Some(s) = c.sessions.get(i).cloned() else { return };
        c.delete_session();
        (i, s)
    };
    ui.go("sessoes");
    ui.changed(Change::Sessions);
    let name = removed.name.clone();
    let back = RefCell::new(Some((i, removed)));
    ui.toast(
        &trf("“{name}” deleted", &[("name", name)]),
        Some((
            tr("Undo"),
            Box::new(move |u: &Rc<Ui>| {
                if let Some((i, s)) = back.borrow_mut().take() {
                    let mut c = u.core.borrow_mut();
                    let i = i.min(c.sessions.len());
                    c.sessions.insert(i, s);
                    drop(c);
                    u.changed(Change::Sessions);
                }
            }),
        )),
    );
}

// ---------------------------------------------------------------- the bottom sheet (medium and narrow windows)

struct Sheet {
    root: gtk::Box,
    holder: gtk::Box,
    h: Cell<Mola>,
    /// 0 closed, 1 half, 2 full
    state: Cell<u8>,
    ticking: Cell<bool>,
    drag_from: Cell<f64>,
}

impl Sheet {
    fn new() -> Rc<Sheet> {
        let root = vbox(0, "ka-folha");
        root.set_valign(Align::End);
        root.set_vexpand(false);
        let handle = hbox(0, "ka-folha-alca");
        handle.set_halign(Align::Fill);
        let grip = hbox(0, "ka-folha-pegador");
        grip.set_halign(Align::Center);
        grip.set_valign(Align::Center);
        grip.set_hexpand(true);
        handle.append(&grip);
        handle.set_cursor_from_name(Some("ns-resize"));
        root.append(&handle);
        let holder = vbox(0, "");
        holder.set_vexpand(true);
        root.append(&holder);
        root.set_visible(false);
        let s = Rc::new(Sheet { root, holder, h: Cell::new(Mola::new(0.0)), state: Cell::new(0), ticking: Cell::new(false), drag_from: Cell::new(0.0) });
        let drag = gtk::GestureDrag::new();
        {
            let s2 = s.clone();
            drag.connect_drag_begin(move |_, _, _| s2.drag_from.set(s2.h.get().x));
            let s2 = s.clone();
            drag.connect_drag_update(move |_, _, dy| {
                let mut m = s2.h.get();
                m.pular((s2.drag_from.get() - dy).max(0.0));
                s2.h.set(m);
                s2.apply();
            });
            let s2 = s.clone();
            drag.connect_drag_end(move |_, _, dy| {
                let h = s2.h.get().x;
                let targets = [0.0, s2.target(1), s2.target(2)];
                // the nearest stop, nudged in the direction of the drag
                let biased = h - dy.signum() * 40.0;
                let st = (0..3).min_by(|a, b| (targets[*a] - biased).abs().total_cmp(&(targets[*b] - biased).abs())).unwrap_or(1) as u8;
                s2.go(st, mov::RAPIDA);
            });
        }
        handle.add_controller(drag);
        s
    }

    fn avail(&self) -> f64 {
        self.root.parent().map(|p| p.height() as f64).unwrap_or(800.0)
    }

    fn target(&self, st: u8) -> f64 {
        let a = self.avail();
        if a < 100.0 {
            return 0.0;
        }
        match st {
            0 => 0.0,
            1 => (a * 0.5).clamp(220.0, 380.0).min(a - 40.0).max(0.0),
            _ => (a - 24.0).max(200.0),
        }
    }

    fn apply(&self) {
        let h = self.h.get().x.max(0.0);
        self.root.set_visible(h > 1.0);
        self.root.set_size_request(-1, h.round() as i32);
    }

    fn go(self: &Rc<Self>, st: u8, kind: mov::Tipo) {
        self.state.set(st);
        let mut m = self.h.get();
        m.ir(self.target(st), kind);
        self.h.set(m);
        if self.ticking.replace(true) {
            return;
        }
        self.root.set_visible(true);
        let s = self.clone();
        let last = Cell::new(0i64);
        // ticks on the parent (the sheet itself may be hidden)
        let anchor: gtk::Widget = self.root.parent().unwrap_or_else(|| self.root.clone().upcast());
        anchor.add_tick_callback(move |_, clock| {
            let t = clock.frame_time();
            let dt = if last.get() == 0 { 1.0 / 60.0 } else { ((t - last.get()) as f64 / 1e6).clamp(0.0, 0.05) };
            last.set(t);
            let mut m = s.h.get();
            m.passo(dt);
            s.h.set(m);
            s.apply();
            if m.parada() {
                s.ticking.set(false);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
}

// ---------------------------------------------------------------- the page

struct Editor {
    ui: Rc<Ui>,
    bar: Rc<Bar>,
    left: gtk::Box,
    right: gtk::Box,
    center: gtk::Overlay,
    sheet: Rc<Sheet>,
    plus: gtk::Button,
    plus_pop: gtk::Popover,
    pop_holder: gtk::Box,
    canvas: Rc<Canvas>,
    inspector: Rc<Inspector>,
    library: Rc<Library>,
    layout: Cell<Option<Layout>>,
}

fn reparent(w: &impl IsA<gtk::Widget>, to: &gtk::Box) {
    if w.parent().as_ref() == Some(to.upcast_ref()) {
        return;
    }
    if let Some(p) = w.parent() {
        if let Some(b) = p.downcast_ref::<gtk::Box>() {
            b.remove(w);
        } else {
            w.unparent();
        }
    }
    to.append(w);
}

impl Editor {
    fn set_layout(self: &Rc<Self>, l: Layout) {
        if self.layout.get() == Some(l) {
            return;
        }
        self.layout.set(Some(l));
        self.bar.set_layout(l);
        let lib = self.library.root.clone();
        let insp = self.inspector.root.clone();
        match l {
            Layout::Wide => {
                reparent(&lib, &self.left);
                reparent(&insp, &self.right);
                self.left.set_visible(true);
                self.right.set_visible(true);
                self.sheet.go(0, mov::RAPIDA);
                self.sheet.root.set_visible(false);
                self.canvas.set_inset(0.0);
            }
            Layout::Medium | Layout::Narrow => {
                if l == Layout::Medium {
                    reparent(&lib, &self.left);
                    self.left.set_visible(true);
                } else {
                    reparent(&lib, &self.pop_holder);
                    self.left.set_visible(false);
                }
                reparent(&insp, &self.sheet.holder);
                self.right.set_visible(false);
                let has_sel = self.ui.core.borrow().sel.is_some();
                self.sheet.go(if has_sel { 1 } else { 0 }, mov::SUAVE);
            }
        }
        self.plus.set_visible(l == Layout::Narrow);
        self.canvas.set_narrow(l == Layout::Narrow);
    }

    fn on_selection(self: &Rc<Self>) {
        if self.layout.get() != Some(Layout::Wide) {
            let has_sel = self.ui.core.borrow().sel.is_some();
            let st = self.sheet.state.get();
            if has_sel && st == 0 {
                self.sheet.go(1, mov::SUAVE);
            } else if !has_sel && st != 0 {
                self.sheet.go(0, mov::SUAVE);
            }
        }
    }

    /// Follows the sheet: the canvas' pill and status and the + button stay above it.
    fn follow_sheet(&self) {
        // the window was resized (or shown for the first time): the open sheet keeps its proportion
        if self.layout.get() != Some(Layout::Wide) && !self.sheet.ticking.get() {
            let want = self.sheet.target(self.sheet.state.get());
            if (self.sheet.h.get().alvo - want).abs() > 1.0 {
                self.sheet.go(self.sheet.state.get(), mov::SUAVE);
            }
        }
        let h = if self.sheet.root.is_visible() { self.sheet.h.get().x.max(0.0) } else { 0.0 };
        self.canvas.set_inset(h);
        self.plus.set_margin_bottom(16 + h as i32);
    }
}

fn keys(ed: &Rc<Editor>, root: &gtk::Box) {
    let k = gtk::EventControllerKey::new();
    let ed = ed.clone();
    k.connect_key_pressed(move |ctl, key, _, state| {
        // typing in a field is not a shortcut
        if let Some(w) = ctl.widget().and_then(|w| w.root()).and_then(|r| r.focus()) {
            if w.is::<gtk::Text>() || w.is::<gtk::TextView>() || w.ancestor(gtk::Entry::static_type()).is_some() || w.is::<gtk::SpinButton>() {
                return glib::Propagation::Proceed;
            }
        }
        let ui = &ed.ui;
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let sel = ui.core.borrow().sel;
        match key {
            gdk::Key::space if !ctrl => {
                let cur = ui.core.borrow().cur;
                ui.toggle(cur);
                glib::Propagation::Stop
            }
            gdk::Key::Delete | gdk::Key::BackSpace | gdk::Key::KP_Delete => {
                if ed.canvas.delete_wire() {
                    return glib::Propagation::Stop;
                }
                if let Some(id) = sel {
                    remove_block(ui, id);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            gdk::Key::d | gdk::Key::D if ctrl => {
                if let Some(id) = sel {
                    duplicate_block(ui, id);
                }
                glib::Propagation::Stop
            }
            gdk::Key::Escape => {
                if sel.is_some() || ed.canvas.wire_sel.get().is_some() {
                    ed.canvas.wire_sel.set(None);
                    ui.core.borrow_mut().sel = None;
                    ui.changed(Change::Selection);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
            _ => glib::Propagation::Proceed,
        }
    });
    root.add_controller(k);
}

/// Picks the session and block asked for by the capture variables (see the module's notes).
fn capture_env(ui: &Rc<Ui>) {
    let Ok(name) = std::env::var("KALANGO_AUDIO_SESSAO") else { return };
    let idx = ui.core.borrow().find_session(&name);
    let Some(i) = idx else { return };
    ui.core.borrow_mut().open(i);
    if std::env::var_os("KALANGO_AUDIO_SESSAO_RODAR").is_some() && !ui.core.borrow().is_running(ui.core.borrow().sessions[i].uid) {
        ui.core.borrow_mut().toggle_run(i);
    }
    if let Ok(kind) = std::env::var("KALANGO_AUDIO_BLOCO") {
        let id = ui.core.borrow().session().and_then(|s| s.blocks.iter().find(|b| b.kind == kind).map(|b| b.id));
        ui.core.borrow_mut().sel = id;
    }
}

pub fn page(ui: &Rc<Ui>) -> gtk::Widget {
    let root = vbox(0, "ka-editor");
    let bar = build_bar(ui);
    root.append(&bar.root);

    let body = hbox(0, "ka-editor-corpo");
    body.set_vexpand(true);
    let left = vbox(0, "ka-editor-esquerda");
    left.set_size_request(248, -1);
    left.set_hexpand(false);
    body.append(&left);
    let center = gtk::Overlay::new();
    center.set_hexpand(true);
    center.set_vexpand(true);
    let canvas = Canvas::new(ui);
    center.set_child(Some(&canvas.root));
    body.append(&center);
    let right = vbox(0, "ka-editor-direita");
    right.set_size_request(320, -1);
    right.set_hexpand(false);
    body.append(&right);
    root.append(&body);

    let sheet = Sheet::new();
    center.add_overlay(&sheet.root);

    // narrow: the library behind a + button
    let plus = gtk::Button::new();
    plus.add_css_class("ka-mais-bloco");
    plus.set_child(Some(&icons::icon("mais", "accent_fg", 18)));
    plus.set_tooltip_text(Some(tr("Add a block")));
    plus.set_halign(Align::Start);
    plus.set_valign(Align::End);
    plus.set_margin_start(24);
    plus.set_margin_bottom(16);
    plus.set_visible(false);
    let plus_pop = gtk::Popover::new();
    plus_pop.add_css_class("ka-biblioteca-pop");
    plus_pop.set_position(gtk::PositionType::Top);
    let pop_holder = vbox(0, "");
    pop_holder.set_size_request(280, 460);
    plus_pop.set_child(Some(&pop_holder));
    plus_pop.set_parent(&plus);
    {
        let p = plus_pop.clone();
        plus.connect_clicked(move |_| p.popup());
    }
    center.add_overlay(&plus);

    let library = Library::new(ui);
    {
        let p = plus_pop.clone();
        library.on_pick(move || p.popdown());
    }
    let inspector = Inspector::new(ui);
    capture_env(ui);

    let ed = Rc::new(Editor {
        ui: ui.clone(),
        bar,
        left,
        right,
        center,
        sheet,
        plus,
        plus_pop,
        pop_holder,
        canvas,
        inspector,
        library,
        layout: Cell::new(None),
    });
    let _ = &ed.plus_pop;
    let _ = &ed.center;
    {
        let w = Rc::downgrade(&ed);
        *ed.canvas.on_press.borrow_mut() = Some(Box::new(move || {
            if let Some(e) = w.upgrade() {
                if e.layout.get() != Some(Layout::Wide) && e.sheet.state.get() == 0 {
                    e.sheet.go(1, mov::SUAVE);
                }
            }
        }));
    }
    ed.set_layout(ui.layout());
    ed.bar.sync(ui);
    ed.canvas.sync();
    ed.inspector.rebuild(false);
    keys(&ed, &root);

    {
        let e = ed.clone();
        ui.on_change(move |ui, c| match c {
            Change::Sessions => {
                e.bar.sync(ui);
                e.canvas.sync();
                e.inspector.on_sessions();
            }
            Change::Selection => {
                e.canvas.sync();
                e.inspector.rebuild(true);
                e.on_selection();
                // the list of programs playing sound is kept fresh while an Application block is open
                let c = ui.core.borrow();
                let app = c.sel.and_then(|id| c.session()?.block(id)).map(|b| b.kind == "app_source").unwrap_or(false);
                c.want_apps.store(app, std::sync::atomic::Ordering::Relaxed);
            }
            Change::Running => {
                show_notices(ui);
                e.bar.tick(ui);
                e.canvas.refresh(true);
            }
            Change::Layout => e.set_layout(ui.layout()),
            _ => {}
        });
    }
    {
        let e = ed.clone();
        ui.on_page(move |ui, name| {
            if name == "editor" {
                e.bar.sync(ui);
                e.canvas.sync();
                e.inspector.rebuild(false);
                e.canvas.below_focus();
            }
        });
    }
    {
        let e = ed.clone();
        let n = Cell::new(0u32);
        ui.on_tick(move |ui| {
            // starts refused by timers, automations or scripts are said on any page
            if !ui.core.borrow().notices.is_empty() {
                show_notices(ui);
            }
            if ui.page() != "editor" {
                return;
            }
            n.set(n.get().wrapping_add(1));
            let running = {
                let c = ui.core.borrow();
                c.session().map(|s| c.is_running(s.uid)).unwrap_or(false)
            };
            e.canvas.tick(running);
            e.inspector.tick();
            e.follow_sheet();
            if n.get().is_multiple_of(6) {
                e.bar.tick(ui);
                e.canvas.refresh(false);
            }
        });
    }
    root.upcast()
}

impl Canvas {
    fn below_focus(&self) {
        if let Some(c) = self.root.child() {
            c.grab_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Session;

    #[test]
    fn summaries_say_the_main_setting() {
        let mut s = Session::new("t");
        let c = s.add_block("compressor", 0.0, 0.0);
        let r = s.add_block("recorder", 6.0, 0.0);
        let v = s.add_block("volume", 12.0, 0.0);
        let i = s.add_block("device_input", 0.0, 6.0);
        let e = s.add_block("graphic_eq", 6.0, 6.0);
        let n = s.add_block("noise_reduction", 6.0, 6.0);
        {
            let b = s.blocks.iter_mut().find(|b| b.id == c).unwrap();
            b.p.set("amount", 0.75);
            b.p.set("speed", 0.6);
            b.p.set("makeup", 1.2);
            let b = s.blocks.iter_mut().find(|b| b.id == r).unwrap();
            b.p.set("depth", 24.0);
            let b = s.blocks.iter_mut().find(|b| b.id == n).unwrap();
            b.p.set("reduction", 18.0);
        }
        let devs = vec![("alsa_input.usb".to_string(), "USB Headset".to_string())];
        assert_eq!(summary(s.block(c).unwrap(), &devs), "Speech · 75 %");
        assert_eq!(summary(s.block(r).unwrap(), &devs), "WAV · 24 bits");
        assert_eq!(summary(s.block(v).unwrap(), &devs), "100 %");
        assert_eq!(summary(s.block(i).unwrap(), &devs), "System default");
        assert_eq!(summary(s.block(e).unwrap(), &devs), "Flat");
        assert_eq!(summary(s.block(n).unwrap(), &devs), "Noise −18 dB");
        s.blocks.iter_mut().find(|b| b.id == i).unwrap().p.set_text("device", "alsa_input.usb");
        assert_eq!(summary(s.block(i).unwrap(), &devs), "USB Headset");
    }

    #[test]
    fn every_kind_has_a_summary_or_a_reason_not_to() {
        for k in crate::model::KINDS {
            let mut s = Session::new("t");
            let id = s.add_block(k.id, 0.0, 0.0);
            let text = summary(s.block(id).unwrap(), &[]);
            assert!(!text.is_empty(), "{} has no summary", k.id);
        }
    }

    #[test]
    fn gain_reduction_follows_the_curve() {
        let mut s = Session::new("t");
        let c = s.add_block("compressor", 0.0, 0.0);
        let b = s.block(c).unwrap().clone();
        // amount 1: threshold −30 dB, 6:1. Quiet input is untouched, loud input is turned down.
        assert_eq!(gain_reduction(&b, -60.0), 0.0);
        let r = gain_reduction(&b, -6.0);
        assert!(r < -15.0 && r > -21.0, "{r}");
        assert_eq!(level_db(1.0), 0.0);
        assert_eq!(level_db(0.0), -60.0);
    }
}
