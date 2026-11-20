//! Settings (design "Ajustes (dispositivos, formatos, pastas)"): sections on the left (Devices, Recording,
//! Transcription, Permissions, Tools, Agent) that jump to their part of the page; each row has its label, a short
//! help and the control on the right; the "Sound system" panel shows what was found, with a teal/gold/red dot and
//! the action that fixes it. Everything is kept in `core.store.prefs` (saved by `Core::tick`).
//!
//! The defaults here are used when a session is made from a template ([`Core::new_with_prefs`]); blocks that exist
//! keep their own settings (the editor changes those).

use super::estados::{self, Level, SoundReport};
use super::widgets::*;
use super::{icons, Change, Layout, Ui};
use crate::core::Core;
use crate::i18n::{n_, tr, trf};
use crate::presets::Prefs;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, Align};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ---------------------------------------------------------------- the defaults applied to new sessions

impl Core {
    /// A session from a template (by index) with the person's defaults applied; returns its index.
    pub fn new_with_prefs(&mut self, t: usize) -> usize {
        let i = self.new_from_template(t);
        self.apply_prefs(i);
        i
    }

    /// Gives the blocks of a (new) session the defaults from Settings: the recorders' folder, quality, file name,
    /// splitting and silence; the transcription model and language; the default devices.
    pub fn apply_prefs(&mut self, idx: usize) {
        let p = self.store.prefs.clone();
        if let Some(s) = self.sessions.get_mut(idx) {
            apply_prefs_to(&p, &mut s.blocks);
        }
    }
}

/// The settings' defaults on a list of blocks (pure: unit-tested).
pub fn apply_prefs_to(p: &Prefs, blocks: &mut [crate::model::Block]) {
    for b in blocks.iter_mut() {
        match b.kind.as_str() {
            "recorder" => {
                if !p.default_folder.trim().is_empty() {
                    b.p.set_text("folder", &p.default_folder);
                }
                if !p.default_format.is_empty() {
                    b.p.set_text("format", &p.default_format);
                }
                if p.sample_rate > 0 {
                    b.p.set("q_rate", p.sample_rate as f64);
                }
                if p.bit_depth > 0 {
                    b.p.set("depth", p.bit_depth as f64);
                }
                if !p.file_name.trim().is_empty() {
                    b.p.set_text("file_name", p.file_name.trim());
                }
                if p.split_long {
                    b.p.set("split_on", 1.0);
                    b.p.set("split_size", split_minutes(p) as f64);
                    b.p.set_text("split_unit", "time*60");
                }
                if p.skip_silence {
                    b.p.set("silence_on", 1.0);
                    b.p.set("silence_action", 1.0);
                }
            }
            "device_input" if !p.default_input.is_empty() && b.p.text("device", "default") == "default" => b.p.set_text("device", &p.default_input),
            "device_output" if !p.default_output.is_empty() && b.p.text("device", "default") == "default" => b.p.set_text("device", &p.default_output),
            _ => {}
        }
    }
}

fn split_minutes(p: &Prefs) -> u32 {
    if p.split_minutes == 0 {
        60
    } else {
        p.split_minutes
    }
}

// ---------------------------------------------------------------- the page

/// (id, glyph, title) of each section, in order.
const SECTIONS: [(&str, &str, &str); 6] = [
    ("dispositivos", "fone", n_("Devices")),
    ("gravacao", "download", n_("Recording")),
    ("transcricao", "doc", n_("Transcription")),
    ("permissoes", "escudo", n_("Permissions")),
    ("ferramentas", "terminal", n_("Tools")),
    ("agente", "spark", n_("Agent")),
];

/// Recording formats: (value kept in the prefs, label msgid, needs ffmpeg).
const FORMATS: [(&str, &str, bool); 6] = [
    ("WAV", n_("WAV · uncompressed"), false),
    ("AIFF", n_("AIFF · uncompressed"), false),
    ("FLAC", n_("FLAC · lossless"), true),
    ("ALAC", n_("ALAC · lossless"), true),
    ("MP3", n_("MP3 · compressed"), true),
    ("AAC", n_("AAC · compressed"), true),
];

const RATES: [u32; 4] = [0, 44100, 48000, 96000];
const DEPTHS: [u32; 3] = [16, 24, 32];
const SPLITS: [u32; 5] = [15, 30, 60, 120, 240];
/// The file name tokens offered as chips: (token, label msgid).
const TOKENS: [(&str, &str); 4] = [("%name", n_("session")), ("%date", n_("date")), ("%time", n_("time")), ("%counter", n_("counter"))];

struct Page {
    scroll: gtk::ScrolledWindow,
    content: gtk::Box,
    nav: gtk::Box,
    nav_buttons: Vec<(String, gtk::Button)>,
    sections: Vec<(String, gtk::Widget)>,
    right: gtk::Box,
    tools_slot: gtk::Box,
    tools_panel: gtk::Box,
    tools_rows: gtk::Box,
    narrow_nav: gtk::ScrolledWindow,
    note_bottom: gtk::Label,
    chips: gtk::Box,
    example: gtk::Label,
    format_help: gtk::Label,
    format_dd: gtk::DropDown,
    voice_status: gtk::Box,
    perm_mic: gtk::Box,
    perm_app: gtk::Box,
    report: RefCell<Option<SoundReport>>,
    jumping: Cell<bool>,
}

fn set_pref(ui: &Rc<Ui>, f: impl FnOnce(&mut Prefs)) {
    f(&mut ui.core.borrow_mut().store.prefs);
    ui.changed(Change::Store);
}

fn prefs(ui: &Rc<Ui>) -> Prefs {
    ui.core.borrow().store.prefs.clone()
}

/// A settings row: title and short help on the left, the control on the right.
fn row(title: &str, help: &str, control: &impl IsA<gtk::Widget>) -> (gtk::Box, gtk::Label) {
    let r = hbox(16, "ka-ajuste-linha");
    let texts = vbox(2, "");
    texts.set_hexpand(true);
    texts.set_valign(Align::Center);
    texts.append(&line(title, "ka-texto"));
    let h = para(help, "ka-fraco");
    h.set_visible(!help.is_empty());
    texts.append(&h);
    r.append(&texts);
    let c = control.as_ref();
    c.set_valign(Align::Center);
    r.append(c);
    (r, h)
}

fn section(id: &str, title: &str, subtitle: &str) -> gtk::Box {
    let s = vbox(8, "ka-ajuste-secao");
    unsafe {
        s.set_data("secao", id.to_string());
    }
    s.append(&label(title, "ka-titulo2"));
    if !subtitle.is_empty() {
        let p = para(subtitle, "ka-fraco");
        p.set_margin_bottom(4);
        s.append(&p);
    }
    s
}

fn dd_width(d: &gtk::DropDown, w: i32) {
    d.set_size_request(w, -1);
}

fn ffmpeg_help(have: bool) -> String {
    if have {
        tr("FLAC, ALAC, MP3 and AAC need ffmpeg (found).").to_string()
    } else {
        tr("FLAC, ALAC, MP3 and AAC need ffmpeg, which was not found: those are recorded as WAV.").to_string()
    }
}

fn format_labels(have_ffmpeg: bool) -> Vec<String> {
    FORMATS
        .iter()
        .map(|(_, l, needs)| if *needs && !have_ffmpeg { trf("{format} (needs ffmpeg)", &[("format", tr(l).to_string())]) } else { tr(l).to_string() })
        .collect()
}

/// The device lists come from the engine (loaded once if the app started without them).
fn devices(ui: &Rc<Ui>) -> Vec<(String, String)> {
    let mut c = ui.core.borrow_mut();
    if c.devices.is_empty() {
        c.devices = crate::audio::list_devices();
    }
    c.devices.clone()
}

fn device_row(ui: &Rc<Ui>, title: &str, help: &str, output: bool) -> gtk::Box {
    let list = devices(ui);
    let cur = {
        let p = prefs(ui);
        if output {
            p.default_output
        } else {
            p.default_input
        }
    };
    let labels: Vec<String> = list.iter().map(|d| d.1.clone()).collect();
    let sel = list.iter().position(|d| d.0 == cur || (cur.is_empty() && d.0 == "default")).unwrap_or(0);
    let dd = dropdown(&labels, sel);
    dd_width(&dd, 260);
    let u = ui.clone();
    dd.connect_selected_notify(move |d| {
        let Some((id, _)) = list.get(d.selected() as usize).cloned() else { return };
        let v = if id == "default" { String::new() } else { id };
        set_pref(&u, |p| if output { p.default_output = v } else { p.default_input = v });
    });
    row(title, help, &dd).0
}

// ---------------- file name chips

fn pattern_of(p: &Prefs) -> String {
    if p.file_name.trim().is_empty() {
        crate::engine::DEFAULT_FILE_NAME.to_string()
    } else {
        p.file_name.trim().to_string()
    }
}

/// "%name %date %time" → its parts, words kept as they are.
pub fn pattern_parts(pattern: &str) -> Vec<String> {
    pattern.split_whitespace().map(str::to_string).collect()
}

fn chip_label(part: &str) -> (String, bool) {
    match TOKENS.iter().find(|(t, _)| *t == part) {
        Some((_, l)) => (tr(l).to_string(), true),
        None => (part.to_string(), false),
    }
}

/// The name a recording would get now with this pattern.
pub fn example_name(pattern: &str, format: &str) -> String {
    let fmt = if format.is_empty() { "WAV" } else { format };
    format!("{}.{}", crate::engine::expand_tokens(pattern, tr("Weekly meeting"), 1), crate::engine::format_ext(fmt))
}

fn save_parts(ui: &Rc<Ui>, page: &Rc<Page>, parts: &[String]) {
    let pat = parts.join(" ");
    set_pref(ui, |p| p.file_name = pat);
    fill_chips(ui, page);
}

fn fill_chips(ui: &Rc<Ui>, page: &Rc<Page>) {
    clear(&page.chips);
    let p = prefs(ui);
    let parts = pattern_parts(&pattern_of(&p));
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            page.chips.append(&label("·", "ka-fraco"));
        }
        let (text, token) = chip_label(part);
        let b = gtk::Button::new();
        b.add_css_class("ka-ficha");
        if !token {
            b.add_css_class("texto");
        }
        let inner = hbox(4, "");
        inner.append(&gtk::Label::new(Some(&text)));
        inner.append(&icons::icon("fechar", "muted", 10));
        b.set_child(Some(&inner));
        b.set_tooltip_text(Some(tr("Remove · drag to reorder")));
        {
            let (u, pg, parts, i) = (ui.clone(), page.clone(), parts.clone(), i);
            b.connect_clicked(move |_| {
                let mut v = parts.clone();
                v.remove(i);
                save_parts(&u, &pg, &v);
            });
        }
        // drag a chip onto another to move it there
        let src = gtk::DragSource::new();
        src.set_actions(gdk::DragAction::MOVE);
        src.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&(i as u32).to_value())));
        b.add_controller(src);
        let drop = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
        {
            let (u, pg, parts) = (ui.clone(), page.clone(), parts.clone());
            drop.connect_drop(move |_, v, _, _| {
                let Ok(from) = v.get::<u32>() else { return false };
                let mut list = parts.clone();
                let from = from as usize;
                if from >= list.len() || from == i {
                    return false;
                }
                let x = list.remove(from);
                list.insert(i.min(list.len()), x);
                let (u, pg) = (u.clone(), pg.clone());
                // rebuilding the chips inside the drop handler would drop the target being used
                glib::idle_add_local_once(move || save_parts(&u, &pg, &list));
                true
            });
        }
        b.add_controller(drop);
        page.chips.append(&b);
    }
    // "+": add a token or a word
    let add = gtk::MenuButton::new();
    add.add_css_class("ka-icone");
    add.set_child(Some(&icons::icon("mais", "muted", 14)));
    add.set_tooltip_text(Some(tr("Add to the name")));
    let pop = gtk::Popover::new();
    pop.add_css_class("menu");
    let items = vbox(2, "");
    for (tok, l) in TOKENS {
        let b = gtk::Button::with_label(tr(l));
        b.add_css_class("ka-item-menu");
        let (u, pg, parts, pop2) = (ui.clone(), page.clone(), parts.clone(), pop.clone());
        b.connect_clicked(move |_| {
            pop2.popdown();
            let mut v = parts.clone();
            v.push(tok.to_string());
            save_parts(&u, &pg, &v);
        });
        items.append(&b);
    }
    let word = entry(tr("A word…"), None);
    {
        let (u, pg, parts, pop2) = (ui.clone(), page.clone(), parts.clone(), pop.clone());
        word.connect_activate(move |e| {
            let w: String = e.text().split_whitespace().collect::<Vec<_>>().join("-");
            if w.is_empty() {
                return;
            }
            pop2.popdown();
            let mut v = parts.clone();
            v.push(w);
            save_parts(&u, &pg, &v);
        });
    }
    items.append(&word);
    let reset = gtk::Button::with_label(tr("Back to the default"));
    reset.add_css_class("ka-item-menu");
    {
        let (u, pg, pop2) = (ui.clone(), page.clone(), pop.clone());
        reset.connect_clicked(move |_| {
            pop2.popdown();
            set_pref(&u, |p| p.file_name.clear());
            fill_chips(&u, &pg);
        });
    }
    items.append(&reset);
    pop.set_child(Some(&items));
    add.set_popover(Some(&pop));
    page.chips.append(&add);
    page.example.set_text(&trf("E.g.: {name}", &[("name", example_name(&pattern_of(&p), &p.default_format))]));
}

// ---------------- tools panel

/// What a tool line's button does.
type Fix = Option<(String, Rc<dyn Fn(&Rc<Ui>)>)>;

/// One line of the "Sound system" panel: dot, name, state, and the action that fixes it.
struct ToolLine {
    level: Level,
    title: String,
    state: String,
    action: Fix,
}

fn copy_action(cmd: &'static str) -> Fix {
    Some((
        tr("Copy the install command").to_string(),
        Rc::new(move |ui: &Rc<Ui>| {
            ui.window.clipboard().set_text(cmd);
            ui.toast(&trf("Copied: {cmd}", &[("cmd", cmd.to_string())]), None);
        }),
    ))
}

fn open_uri(ui: &Rc<Ui>, uri: &str) {
    gtk::UriLauncher::new(uri).launch(Some(&ui.window), gio::Cancellable::NONE, |_| {});
}

fn tool_lines(r: &SoundReport, p: &Prefs) -> Vec<ToolLine> {
    let mut v = vec![];
    v.push(if r.sound_ok() {
        let state = match r.pw_rate {
            Some(rate) => trf("Connected · {rate} kHz", &[("rate", format!("{}", rate as f64 / 1000.0).replace('.', if crate::i18n::system_locale().is_some() { "," } else { "." }))]),
            None => tr("Connected").to_string(),
        };
        ToolLine { level: Level::Good, title: r.pw_version.as_ref().map(|v| format!("PipeWire {v}")).unwrap_or_else(|| "PipeWire".into()), state, action: None }
    } else {
        ToolLine {
            level: Level::Bad,
            title: "PipeWire".into(),
            state: tr("Not answering").to_string(),
            action: Some((
                tr("Restart the sound").to_string(),
                Rc::new(|ui: &Rc<Ui>| {
                    let res = estados::restart_sound();
                    match res {
                        Ok(()) => ui.toast(tr("The sound services were restarted."), None),
                        Err(e) => ui.toast(&trf("Could not restart the sound: {err}", &[("err", e)]), None),
                    }
                }),
            )),
        }
    });
    let capture = r.has("pw-record") && r.has("pw-dump");
    v.push(ToolLine {
        level: if capture { Level::Good } else { Level::Bad },
        title: tr("App capture").to_string(),
        state: if capture { tr("pw-record and pw-dump found").to_string() } else { tr("pw-record or pw-dump missing: apps cannot be captured").to_string() },
        action: if capture { None } else { copy_action("sudo apt install pipewire-bin") },
    });
    v.push(ToolLine {
        level: if r.has("ffmpeg") { Level::Good } else { Level::Warn },
        title: "ffmpeg".into(),
        state: if r.has("ffmpeg") { tr("Found · MP3, AAC, ALAC, FLAC, streams").to_string() } else { tr("Not found: only WAV and AIFF, no streams").to_string() },
        action: if r.has("ffmpeg") { None } else { copy_action("sudo apt install ffmpeg") },
    });
    v.push(ToolLine {
        level: if r.has("ffprobe") { Level::Good } else { Level::Warn },
        title: "ffprobe".into(),
        state: if r.has("ffprobe") { tr("Found · durations of compressed files").to_string() } else { tr("Not found: compressed files show no duration").to_string() },
        action: if r.has("ffprobe") { None } else { copy_action("sudo apt install ffmpeg") },
    });
    v.push(match crate::voz::status() {
        crate::voz::Status::Ready => ToolLine {
            level: Level::Good,
            title: tr("Local transcription").to_string(),
            state: tr("KalangoOS voice model (LFM3-Audio-2B) ready").to_string(),
            action: None,
        },
        crate::voz::Status::NoAgent => ToolLine {
            level: Level::Warn,
            title: tr("Local transcription").to_string(),
            state: tr("The KalangoOS agent (kalango-agente) was not found").to_string(),
            action: None,
        },
        crate::voz::Status::NoModel => ToolLine {
            level: Level::Warn,
            title: tr("Local transcription").to_string(),
            state: tr("The voice model is not installed — get it in the App Center").to_string(),
            action: None,
        },
    });
    v.push(ToolLine {
        level: if r.has("gio") { Level::Good } else { Level::Warn },
        title: "gio".into(),
        state: if r.has("gio") { tr("Found · deleted recordings go to the trash").to_string() } else { tr("Not found: recordings cannot go to the trash").to_string() },
        action: if r.has("gio") { None } else { copy_action("sudo apt install libglib2.0-bin") },
    });
    v.push(ToolLine {
        level: if r.ladspa > 0 { Level::Good } else { Level::Warn },
        title: tr("LADSPA plug-ins").to_string(),
        state: if r.ladspa > 0 { trf("{n} found", &[("n", r.ladspa.to_string())]) } else { tr("None installed").to_string() },
        action: if r.ladspa > 0 { None } else { copy_action("sudo apt install swh-plugins") },
    });
    v
}

fn fill_tools(ui: &Rc<Ui>, page: &Rc<Page>) {
    clear(&page.tools_rows);
    let Some(r) = page.report.borrow().clone() else {
        page.tools_rows.append(&label(tr("Checking…"), "ka-fraco"));
        return;
    };
    let p = prefs(ui);
    for t in tool_lines(&r, &p) {
        let row = hbox(8, "ka-ferramenta");
        let d = dot(t.level.token());
        d.set_valign(Align::Start);
        d.set_margin_top(5);
        row.append(&d);
        let txt = vbox(2, "");
        txt.set_hexpand(true);
        txt.append(&line(&t.title, "ka-texto"));
        txt.append(&para(&t.state, "ka-fraco"));
        if let Some((l, f)) = t.action {
            let b = button(&l, "secundario", None);
            b.set_halign(Align::Start);
            b.set_margin_top(8);
            let u = ui.clone();
            b.connect_clicked(move |_| f(&u));
            txt.append(&b);
        }
        row.append(&txt);
        page.tools_rows.append(&row);
    }
    // the format help and the transcription status follow the findings
    page.format_help.set_text(&ffmpeg_help(r.has("ffmpeg")));
    clear(&page.voice_status);
    let ready = crate::voz::status() == crate::voz::Status::Ready;
    page.voice_status.append(&dot(if ready { "teal" } else { "gold" }));
    page.voice_status.append(&label(if ready { tr("Ready") } else { tr("Not installed — App Center") }, "ka-texto2"));
}

fn refresh_report(ui: &Rc<Ui>, page: &Rc<Page>) {
    let (u, p) = (Rc::downgrade(ui), page.clone());
    estados::probe_then(move |r| {
        let Some(u) = u.upgrade() else { return };
        let have = r.has("ffmpeg");
        *p.report.borrow_mut() = Some(r);
        // relabel the formats when ffmpeg came or went
        let sel = p.format_dd.selected();
        let labels = format_labels(have);
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        p.format_dd.set_model(Some(&gtk::StringList::new(&refs)));
        p.format_dd.set_selected(sel);
        fill_tools(&u, &p);
    });
}

/// Microphone and app capture: what the running sessions report (an error on a block), or how it works.
fn fill_permissions(ui: &Rc<Ui>, page: &Rc<Page>) {
    let errors = |kinds: &[&str]| -> (bool, Option<String>) {
        let c = ui.core.borrow();
        let mut running = false;
        for s in &c.sessions {
            if !c.is_running(s.uid) {
                continue;
            }
            for b in s.blocks.iter().filter(|b| kinds.contains(&b.kind.as_str())) {
                running = true;
                if let Some(e) = c.block_error(s.uid, b.id) {
                    return (true, Some(format!("{} · {e}", s.name)));
                }
            }
        }
        (running, None)
    };
    for (bx, kinds) in [(&page.perm_mic, &["device_input", "voice_denoise"][..]), (&page.perm_app, &["app_source", "system_sound"][..])] {
        clear(bx);
        let (running, err) = errors(kinds);
        let (tok, text) = match (running, err) {
            (_, Some(e)) => ("red", e),
            (true, None) => ("teal", tr("Working in the running sessions").to_string()),
            (false, None) => ("gold", tr("Not checked yet: the system asks the first time a session uses it").to_string()),
        };
        bx.append(&dot(tok));
        bx.append(&para(&text, "ka-texto2"));
    }
}

fn audio_editors() -> Vec<(String, String)> {
    // (command, name): the installed apps that open audio files
    let mut v: Vec<(String, String)> = vec![];
    for t in ["audio/x-wav", "audio/flac", "audio/mpeg"] {
        for a in gio::AppInfo::all_for_type(t) {
            let exe = a.executable().to_string_lossy().to_string();
            if exe.is_empty() || v.iter().any(|x| x.0 == exe) {
                continue;
            }
            v.push((exe, a.display_name().to_string()));
        }
    }
    v
}

fn build_devices(ui: &Rc<Ui>) -> gtk::Box {
    let s = section("dispositivos", tr("Devices"), tr("Used by new Microphone and Audio output blocks that have no device of their own."));
    s.append(&device_row(ui, tr("Default input"), tr("Used by Microphone blocks with no device chosen."), false));
    s.append(&device_row(ui, tr("Default output"), tr("Used by Audio output blocks with no device chosen."), true));
    s
}

fn build_recording(ui: &Rc<Ui>, page_cell: &Rc<RefCell<Option<Rc<Page>>>>) -> (gtk::Box, gtk::DropDown, gtk::Label, gtk::Box, gtk::Label) {
    let p = prefs(ui);
    let s = section("gravacao", tr("Recording"), tr("How and where files are saved. Each Recorder block can change this just for itself."));
    let have = crate::media::have("ffmpeg");
    let fsel = FORMATS.iter().position(|f| f.0 == p.default_format).unwrap_or(0);
    let fmt = dropdown(&format_labels(have), fsel);
    dd_width(&fmt, 260);
    {
        let (u, pc) = (ui.clone(), page_cell.clone());
        fmt.connect_selected_notify(move |d| {
            let v = FORMATS.get(d.selected() as usize).map(|f| f.0.to_string()).unwrap_or_default();
            set_pref(&u, |p| p.default_format = v);
            if let Some(pg) = pc.borrow().as_ref() {
                fill_chips(&u, pg);
            }
        });
    }
    let (r, fhelp) = row(tr("Default format"), &ffmpeg_help(have), &fmt);
    s.append(&r);

    // quality: rate and bits
    let q = hbox(8, "");
    let rate_labels: Vec<String> = RATES.iter().map(|r| if *r == 0 { tr("Automatic").to_string() } else { format!("{} kHz", *r as f64 / 1000.0) }).collect();
    let rate = dropdown(&rate_labels, RATES.iter().position(|r| *r == p.sample_rate).unwrap_or(0));
    dd_width(&rate, 126);
    let depth_labels: Vec<String> = DEPTHS.iter().map(|d| if *d == 32 { tr("32 bits float").to_string() } else { trf("{n} bits", &[("n", d.to_string())]) }).collect();
    let depth = dropdown(&depth_labels, DEPTHS.iter().position(|d| *d == p.bit_depth).unwrap_or(0));
    dd_width(&depth, 126);
    {
        let u = ui.clone();
        rate.connect_selected_notify(move |d| {
            let v = RATES.get(d.selected() as usize).copied().unwrap_or(0);
            set_pref(&u, |p| p.sample_rate = v);
        });
        let u = ui.clone();
        depth.connect_selected_notify(move |d| {
            let v = DEPTHS.get(d.selected() as usize).copied().unwrap_or(16);
            set_pref(&u, |p| p.bit_depth = v);
        });
    }
    q.append(&rate);
    q.append(&depth);
    s.append(&row(tr("Quality"), tr("Rate and bits of the file; “Automatic” follows the session. MP3 and AAC use their own bit rate."), &q).0);

    // the folder
    let fbox = hbox(8, "");
    let path = hbox(8, "ka-caminho");
    path.append(&icons::icon("pasta", "muted", 14));
    let pl = line(&p.default_folder, "ka-texto2");
    path.append(&pl);
    path.set_size_request(240, -1);
    fbox.append(&path);
    let change = button(tr("Change…"), "secundario", None);
    {
        let (u, pl) = (ui.clone(), pl.clone());
        change.connect_clicked(move |_| {
            let fd = gtk::FileDialog::builder().title(tr("Recordings folder")).modal(true).build();
            let cur = crate::core::expand_home(&prefs(&u).default_folder);
            if cur.is_dir() {
                fd.set_initial_folder(Some(&gio::File::for_path(&cur)));
            }
            let (u2, pl2) = (u.clone(), pl.clone());
            fd.select_folder(Some(&u.window), gio::Cancellable::NONE, move |res| {
                let Ok(f) = res else { return };
                let Some(path) = f.path() else { return };
                let home = std::env::var("HOME").unwrap_or_default();
                let s = match path.strip_prefix(&home) {
                    Ok(r) if !home.is_empty() => format!("~/{}", r.display()),
                    _ => path.display().to_string(),
                };
                pl2.set_text(&s);
                {
                    let mut c = u2.core.borrow_mut();
                    c.store.prefs.default_folder = s;
                    c.rec_scanned = None;
                }
                u2.changed(Change::Store);
                u2.changed(Change::Recordings);
            });
        });
    }
    fbox.append(&change);
    s.append(&row(tr("Recordings folder"), tr("The library reads this folder; moving files out of it takes them off the list."), &fbox).0);

    // the file name
    let chips = hbox(4, "ka-fichas");
    chips.set_size_request(340, -1);
    let (r, ex) = row(tr("File name"), "", &chips);
    ex.set_visible(true);
    s.append(&r);

    // split, silence
    let split = hbox(8, "");
    let mins: Vec<String> = SPLITS.iter().map(|m| trf("every {n} min", &[("n", m.to_string())])).collect();
    let mdd = dropdown(&mins, SPLITS.iter().position(|m| *m == split_minutes(&p)).unwrap_or(2));
    dd_width(&mdd, 150);
    mdd.set_sensitive(p.split_long);
    let sw = switch(p.split_long);
    split.append(&mdd);
    split.append(&sw);
    let (srow, shelp) = row(tr("Split long files"), "", &split);
    let help_for = |m: u32| trf("Starts a new file every {n} min, without losing sound.", &[("n", m.to_string())]);
    shelp.set_text(&help_for(split_minutes(&p)));
    shelp.set_visible(true);
    {
        let (u, mdd2) = (ui.clone(), mdd.clone());
        sw.connect_active_notify(move |s| {
            let on = s.is_active();
            mdd2.set_sensitive(on);
            set_pref(&u, |p| p.split_long = on);
        });
        let u = ui.clone();
        mdd.connect_selected_notify(move |d| {
            let m = SPLITS.get(d.selected() as usize).copied().unwrap_or(60);
            shelp.set_text(&help_for(m));
            set_pref(&u, |p| p.split_minutes = m);
        });
    }
    s.append(&srow);
    let sil = switch(p.skip_silence);
    {
        let u = ui.clone();
        sil.connect_active_notify(move |s| {
            let on = s.is_active();
            set_pref(&u, |p| p.skip_silence = on);
        });
    }
    s.append(
        &row(
            tr("Skip silence"),
            &trf("Pauses when it stays below {db} dB for {s} s and resumes by itself.", &[("db", format!("{}", crate::engine::SILENCE_DB).replace('-', "−")), ("s", crate::engine::SILENCE_SECONDS.to_string())]),
            &sil,
        )
        .0,
    );

    // the audio editor
    let eds = audio_editors();
    let cur = p.audio_editor.trim().to_string();
    let mut labels = vec![tr("None").to_string()];
    let mut cmds = vec![String::new()];
    for (c, n) in &eds {
        labels.push(n.clone());
        cmds.push(c.clone());
    }
    if !cur.is_empty() && !cmds.contains(&cur) {
        labels.push(cur.clone());
        cmds.push(cur.clone());
    }
    let ed = dropdown(&labels, cmds.iter().position(|c| *c == cur).unwrap_or(0));
    dd_width(&ed, 260);
    {
        let u = ui.clone();
        ed.connect_selected_notify(move |d| {
            let v = cmds.get(d.selected() as usize).cloned().unwrap_or_default();
            set_pref(&u, |p| p.audio_editor = v);
        });
    }
    let help = if eds.is_empty() { tr("No installed app says it opens audio files.") } else { tr("Opens recordings from the Recordings page and the “open in the editor” script.") };
    let (er, _) = row(tr("Audio editor"), help, &ed);
    s.append(&er);
    (s, fmt, fhelp, chips, ex)
}

fn build_transcription(_ui: &Rc<Ui>) -> (gtk::Box, gtk::Box) {
    let s = section("transcricao", tr("Transcription"), tr("Transcription runs on this computer with KalangoOS’s voice model (LFM3-Audio-2B); nothing leaves it."));
    let status = hbox(8, "");
    status.append(&label(tr("Checking…"), "ka-fraco"));
    s.append(&row(tr("Voice model"), tr("The same local model the system uses for dictation. Install it in the App Center."), &status).0);
    let lang = label(tr("The system’s language"), "ka-texto2");
    s.append(&row(tr("Language"), tr("Transcripts follow the language chosen in the system’s settings."), &lang).0);
    (s, status)
}

fn build_permissions(ui: &Rc<Ui>) -> (gtk::Box, gtk::Box, gtk::Box) {
    let s = section("permissoes", tr("Permissions"), tr("The system decides who may hear the microphone and the apps; Kalango Audio only asks."));
    let mk = |title: &str| {
        let st = hbox(8, "");
        st.set_size_request(300, -1);
        let open = button(tr("Privacy settings…"), "secundario", None);
        let u = ui.clone();
        open.connect_clicked(move |_| {
            if !estados::open_privacy() {
                u.toast(tr("The privacy settings are part of KalangoOS and were not found here."), None);
            }
        });
        let c = hbox(12, "");
        c.append(&st);
        c.append(&open);
        (row(title, "", &c).0, st)
    };
    let (r1, mic) = mk(tr("Microphone"));
    let (r2, app) = mk(tr("App capture"));
    s.append(&r1);
    s.append(&r2);
    (s, mic, app)
}

fn build_tools(ui: &Rc<Ui>) -> (gtk::Box, gtk::Box, gtk::Box, gtk::Box) {
    let s = section("ferramentas", tr("Tools"), tr("The programs Kalango Audio uses to capture, convert and transcribe."));
    let slot = vbox(0, "");
    s.append(&slot);
    let panel = vbox(12, "ka-cartao ka-sistema-som");
    let head = hbox(8, "");
    head.append(&caps(tr("Sound system")));
    head.append(&spacer());
    let again = icon_button("reiniciar", "muted", 14, tr("Check again"));
    head.append(&again);
    panel.append(&head);
    let rows = vbox(12, "");
    rows.append(&label(tr("Checking…"), "ka-fraco"));
    panel.append(&rows);
    let _ = ui;
    (s, slot, panel, rows)
}

fn build_agent(ui: &Rc<Ui>) -> gtk::Box {
    let s = section("agente", tr("Agent"), tr("Requests typed in the search (Ctrl K) go to the app’s own planner."));
    let try_it = button(tr("Try a request"), "secundario", Some("spark"));
    {
        let u = ui.clone();
        try_it.connect_clicked(move |_| super::agente::ask(&u, ""));
    }
    s.append(
        &row(
            tr("App planner"),
            tr("It turns a request into a plan with Kalango Audio’s own tools (create a session, schedule, record, transcribe) and waits for your approval. It never deletes anything."),
            &try_it,
        )
        .0,
    );
    let st = hbox(8, "");
    st.append(&dot("gold"));
    st.append(&label(tr("Not yet"), "ka-texto2"));
    s.append(&row(tr("KalangoOS agent"), tr("Handing requests to the system’s agent (summaries, free-form requests) is not connected yet."), &st).0);
    s
}

fn sync_nav(page: &Rc<Page>) {
    if page.jumping.get() {
        return;
    }
    let v = page.scroll.vadjustment().value();
    let mut cur = page.sections.first().map(|s| s.0.clone()).unwrap_or_default();
    for (id, w) in page.sections.iter().filter(|s| s.1.height() > 0) {
        if let Some(p) = w.compute_point(&page.content, &gtk::graphene::Point::new(0.0, 0.0)) {
            if (p.y() as f64) <= v + 60.0 {
                cur = id.clone();
            }
        }
    }
    // at the very bottom the last section is the one being read
    let adj = page.scroll.vadjustment();
    if adj.upper() - adj.page_size() - v < 4.0 && adj.upper() > adj.page_size() + 1.0 {
        if let Some(l) = page.sections.last() {
            cur = l.0.clone();
        }
    }
    for (id, b) in &page.nav_buttons {
        if *id == cur {
            b.add_css_class("ativo");
        } else {
            b.remove_css_class("ativo");
        }
    }
}

fn jump(page: &Rc<Page>, id: &str) {
    let Some((_, w)) = page.sections.iter().find(|s| s.0 == id) else { return };
    let Some(p) = w.compute_point(&page.content, &gtk::graphene::Point::new(0.0, 0.0)) else { return };
    let adj = page.scroll.vadjustment();
    let target = ((p.y() as f64) - 8.0).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0));
    for (i, b) in &page.nav_buttons {
        if i == id {
            b.add_css_class("ativo");
        } else {
            b.remove_css_class("ativo");
        }
    }
    // a short glide with the suave spring
    page.jumping.set(true);
    let mut m = kalango_movimento::Mola::new(adj.value());
    m.ir(target, kalango_movimento::SUAVE);
    let m = RefCell::new(m);
    let pg = page.clone();
    page.scroll.add_tick_callback(move |_, _| {
        let mut s = m.borrow_mut();
        s.passo(1.0 / 60.0);
        pg.scroll.vadjustment().set_value(s.x);
        if s.parada() {
            pg.jumping.set(false);
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn place(ui: &Rc<Ui>, page: &Rc<Page>) {
    let l = ui.layout();
    // the "Sound system" panel: on the right when wide, inside Tools otherwise
    let want_right = l == Layout::Wide;
    if let Some(parent) = page.tools_panel.parent() {
        let in_right = parent == *page.right.upcast_ref::<gtk::Widget>();
        if in_right != want_right {
            if let Some(b) = parent.downcast_ref::<gtk::Box>() {
                b.remove(&page.tools_panel);
            }
        }
    }
    if page.tools_panel.parent().is_none() {
        if want_right {
            page.right.prepend(&page.tools_panel);
        } else {
            page.tools_slot.append(&page.tools_panel);
        }
    }
    page.right.set_visible(want_right);
    page.note_bottom.set_visible(!want_right);
    page.nav.set_visible(l != Layout::Narrow);
    page.narrow_nav.set_visible(l == Layout::Narrow);
}

pub fn page(ui: &Rc<Ui>) -> gtk::Widget {
    let root = hbox(0, "ka-ajustes");
    // the sections
    let nav = vbox(2, "ka-ajustes-nav");
    nav.set_size_request(220, -1);
    let t = label(tr("Settings"), "ka-titulo2 ka-ajustes-titulo");
    nav.append(&t);
    let mut nav_buttons = vec![];
    let narrow_row = hbox(6, "");
    let mut narrow_buttons = vec![];
    for (id, glyph, title) in SECTIONS {
        let b = gtk::Button::new();
        b.add_css_class("ka-ajustes-item");
        let r = hbox(10, "");
        r.append(&icons::icon(glyph, "text2", 16));
        r.append(&label(tr(title), "ka-texto"));
        b.set_child(Some(&r));
        nav.append(&b);
        nav_buttons.push((id.to_string(), b));
        let nb = gtk::Button::with_label(tr(title));
        nb.add_css_class("ka-filtro");
        narrow_row.append(&nb);
        narrow_buttons.push((id, nb));
    }
    root.append(&nav);
    let narrow_nav = gtk::ScrolledWindow::new();
    narrow_nav.set_vscrollbar_policy(gtk::PolicyType::Never);
    narrow_nav.set_child(Some(&narrow_row));
    narrow_nav.add_css_class("ka-ajustes-nav-estreita");

    let content = vbox(24, "ka-ajustes-conteudo");
    let page_cell: Rc<RefCell<Option<Rc<Page>>>> = Rc::new(RefCell::new(None));
    let dev = build_devices(ui);
    let (rec, format_dd, format_help, chips, example) = build_recording(ui, &page_cell);
    let (tra, voice_status) = build_transcription(ui);
    let (perm, perm_mic, perm_app) = build_permissions(ui);
    let (tools, tools_slot, tools_panel, tools_rows) = build_tools(ui);
    let agent = build_agent(ui);
    let note = para(
        tr("These settings are used for new sessions and new blocks. Blocks that already exist keep their own settings (change them in the editor); a session that is recording keeps what it started with until it stops."),
        "ka-fraco",
    );
    let sections: Vec<(String, gtk::Widget)> = vec![
        ("dispositivos".into(), dev.clone().upcast()),
        ("gravacao".into(), rec.clone().upcast()),
        ("transcricao".into(), tra.clone().upcast()),
        ("permissoes".into(), perm.clone().upcast()),
        ("ferramentas".into(), tools.clone().upcast()),
        ("agente".into(), agent.clone().upcast()),
    ];
    for (_, w) in &sections {
        content.append(w);
    }
    // the note goes under the sections when the right column gives way
    let note_bottom = para(tr("These settings are used for new sessions and new blocks. Blocks that already exist keep their own settings (change them in the editor); a session that is recording keeps what it started with until it stops."), "ka-fraco");
    content.append(&note_bottom);
    let center = vbox(0, "");
    center.set_hexpand(true);
    center.append(&narrow_nav);
    let scroll = scroller(&content);
    center.append(&scroll);
    root.append(&center);

    let right = vbox(12, "ka-ajustes-direita");
    right.set_size_request(376, -1);
    right.append(&note.clone());
    root.append(&right);

    let page = Rc::new(Page {
        scroll: scroll.clone(),
        content: content.clone(),
        nav,
        nav_buttons,
        sections,
        right,
        tools_slot,
        tools_panel: tools_panel.clone(),
        tools_rows,
        narrow_nav,
        note_bottom,
        chips,
        example,
        format_help,
        format_dd,
        voice_status,
        perm_mic,
        perm_app,
        report: RefCell::new(None),
        jumping: Cell::new(false),
    });
    *page_cell.borrow_mut() = Some(page.clone());
    for (id, b) in &page.nav_buttons {
        let (p, id) = (page.clone(), id.clone());
        b.connect_clicked(move |_| jump(&p, &id));
    }
    for (id, b) in narrow_buttons {
        let p = page.clone();
        b.connect_clicked(move |_| jump(&p, id));
    }
    {
        let p = page.clone();
        scroll.vadjustment().connect_value_changed(move |_| sync_nav(&p));
        let p = page.clone();
        scroll.vadjustment().connect_changed(move |_| sync_nav(&p));
    }
    if let Some(again) = tools_panel.first_child().and_then(|h| h.last_child()).and_downcast::<gtk::Button>() {
        let (u, p) = (ui.clone(), page.clone());
        again.connect_clicked(move |_| {
            *p.report.borrow_mut() = None;
            fill_tools(&u, &p);
            refresh_report(&u, &p);
        });
    }
    fill_chips(ui, &page);
    fill_permissions(ui, &page);
    fill_tools(ui, &page);
    place(ui, &page);
    sync_nav(&page);
    {
        let p = page.clone();
        ui.on_change(move |ui, c| match c {
            Change::Layout => place(ui, &p),
            Change::Running => fill_permissions(ui, &p),
            _ => {}
        });
    }
    {
        let p = page.clone();
        ui.on_page(move |ui, name| {
            if name == "ajustes" {
                refresh_report(ui, &p);
                fill_permissions(ui, &p);
                sync_nav(&p);
            }
        });
    }
    root.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::templates;

    #[test]
    fn new_sessions_take_the_settings() {
        let p = Prefs {
            default_folder: "~/Gravações".into(),
            default_format: "FLAC".into(),
            sample_rate: 44100,
            bit_depth: 24,
            file_name: "%name %date".into(),
            split_long: true,
            split_minutes: 30,
            skip_silence: true,
            default_input: "hw:1".into(),
            ..Default::default()
        };
        let t = templates().into_iter().find(|t| t.name == "Online meeting").unwrap();
        let mut s = t.instantiate();
        apply_prefs_to(&p, &mut s.blocks);
        let rec = s.blocks.iter().find(|b| b.kind == "recorder").unwrap();
        assert_eq!(rec.p.text("folder", ""), "~/Gravações");
        assert_eq!(rec.p.text("format", ""), "FLAC");
        assert_eq!(rec.p.num("q_rate", 0.0), 44100.0);
        assert_eq!(rec.p.num("depth", 0.0), 24.0);
        assert_eq!(rec.p.text("file_name", ""), "%name %date");
        assert_eq!((rec.p.num("split_on", 0.0), rec.p.num("split_size", 0.0), rec.p.text("split_unit", "")), (1.0, 30.0, "time*60".to_string()));
        assert_eq!(rec.p.num("silence_on", 0.0), 1.0);
        let mic = s.blocks.iter().find(|b| b.kind == "device_input").unwrap();
        assert_eq!(mic.p.text("device", ""), "hw:1");
    }

    #[test]
    fn empty_settings_change_nothing() {
        let t = templates().into_iter().find(|t| t.name == "Record an app").unwrap();
        let mut s = t.instantiate();
        let before = s.blocks.clone();
        apply_prefs_to(&Prefs::default(), &mut s.blocks);
        assert_eq!(s.blocks, before);
    }

    #[test]
    fn file_name_example() {
        assert_eq!(pattern_parts("%name  %date x"), vec!["%name", "%date", "x"]);
        let e = example_name("%name %counter", "FLAC");
        assert_eq!(e, "Weekly meeting 1.flac");
        assert!(example_name("%date", "").ends_with(".wav"));
    }
}
