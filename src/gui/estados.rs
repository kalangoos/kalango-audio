//! The app's states outside the pages: first use (design "Primeiro uso"), the sound system missing (left half of
//! "Estados"), the checks behind them (shared with Settings › Tools), the error toasts of running recorders (disk full,
//! recorder errors), and the small dialogs (quit while recording, import, export, about).
//!
//! `KALANGO_AUDIO_SEM_SOM=1` makes [`sound_available`] say no (tests and captures of the "sem-som" page).

use super::widgets::*;
use super::{icons, Change, Layout, Ui};
use crate::core::Core;
use crate::i18n::{tr, trf};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, Align};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A filled-in app for screenshots and trying the screens (`KALANGO_AUDIO_DEMO=1`): sessions made from the
/// templates, a schedule, and short recordings (a tone shaped like speech) with transcripts, all in a temporary
/// folder — nothing is read from or written to the person's own folders. `KALANGO_AUDIO_DEMO=rodando` also starts a
/// session made of a test tone, so the live meters move.
pub fn demo_core() -> Core {
    use crate::model::*;
    let dir = std::env::var("KALANGO_AUDIO_DEMO_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("kalango-audio-demo"));
    let _ = std::fs::create_dir_all(&dir);
    let folder = dir.to_string_lossy().to_string();
    let tpl = templates();
    let make = |name: &str, from: &str| {
        let mut s = tpl.iter().find(|t| t.name == from).map(|t| t.instantiate()).unwrap_or_else(|| Session::new(name));
        s.name = name.to_string();
        for b in s.blocks.iter_mut().filter(|b| b.kind == "recorder" || b.kind == "transcribe") {
            b.p.set_text("folder", &folder);
        }
        s
    };
    let mut sessions = vec![
        make("Reunião semanal", "Online meeting"),
        make("Ouvir mais alto", "Listen louder"),
        make("Kalango Cast", "Remote podcast"),
        make("Narração de vídeos", "Clean narration"),
        make("Rádio da comunidade", "Online radio"),
    ];
    sessions[4].timers.push(Timer { days: 0b0111110, start: 20 * 60, minutes: 120, on: true, mode: 1, date: 0, quit_sources: true });
    sessions[0].timers.push(Timer { days: 0b0000010, start: 14 * 60, minutes: 60, on: true, mode: 1, date: 0, quit_sources: false });
    sessions[0].automations.push(Automation { event: "recording_finished".into(), trigger: 0, minutes: 0, script: crate::script::BUILTIN[5].0.into(), on: true });
    let mut tone = Session::new("Teste de som");
    let a = tone.add_block("test_tone", 2.0, 2.0);
    let v = tone.add_block("volume", 8.0, 2.0);
    let r = tone.add_block("recorder", 14.0, 2.0);
    tone.connect(a, v);
    tone.connect(v, r);
    for b in tone.blocks.iter_mut() {
        if b.id == r {
            b.p.set_text("folder", &folder);
            b.p.set_text("file_name", "Teste de som %time");
        }
        if b.id == a {
            b.p.set("level", 0.3);
        }
    }
    sessions.insert(0, tone);
    // recordings: a tone that swells like speech, a few seconds each, dated over the week
    let now = std::time::SystemTime::now();
    let files = [
        ("Reunião semanal 2026-11-19 14.02.10.wav", 0u64, 6.0f32, true),
        ("Narração de vídeos 2026-11-19 10.15.00.wav", 3, 4.0, false),
        ("Kalango Cast 2026-11-18 19.30.00.wav", 20, 5.0, true),
        ("Rádio da comunidade 2026-11-18 20.00.00.wav", 19, 5.0, false),
        ("Reunião semanal 2026-11-16 14.00.00.wav", 72, 6.0, true),
    ];
    for (i, (name, hours_ago, secs, transcript)) in files.iter().enumerate() {
        let p = dir.join(name);
        if !p.exists() {
            let spec = hound::WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            if let Ok(mut w) = hound::WavWriter::create(&p, spec) {
                let n = (secs * 8000.0) as usize;
                for k in 0..n {
                    let t = k as f32 / 8000.0;
                    let env = (0.35 + 0.65 * ((t * (1.7 + i as f32 * 0.3)).sin() * (t * 0.6 + i as f32).cos()).abs()) * (0.6 + 0.4 * ((t * 13.0).sin() * 0.5 + 0.5));
                    let s = (t * 220.0 * std::f32::consts::TAU).sin() * env * 0.5;
                    let _ = w.write_sample((s * 32767.0) as i16);
                }
                let _ = w.finalize();
            }
            if *transcript {
                let _ = std::fs::write(
                    p.with_extension("txt"),
                    "[00:12] Você: Bom dia! Vamos começar pela pauta de sexta?\n[00:19] Participante 2: Pode ser. Eu trouxe os números da semana.\n[16:44] Você: Então fechamos a pauta de sexta com esses três pontos.\n[16:52] Participante 3: Combinado, eu mando o resumo depois.\n",
                );
            }
        }
        let when = now - std::time::Duration::from_secs(hours_ago * 3600);
        if let Ok(f) = std::fs::File::options().write(true).open(&p) {
            let _ = f.set_modified(when);
        }
    }
    let mut c = Core::ephemeral(sessions);
    c.watch_apps();
    c.store.prefs.default_folder = folder;
    c.store.prefs.welcomed = true;
    if let Some(first) = c.recs.first().map(|r| r.path.clone()) {
        c.set_tags(&first, vec!["reunião".into()]);
    }
    c.scan_recordings();
    let paths: Vec<_> = c.recs.iter().map(|r| (r.path.clone(), r.name.clone())).collect();
    for (p, n) in paths {
        let tag = if n.starts_with("Reunião") { "reunião" } else if n.starts_with("Kalango") { "podcast" } else if n.starts_with("Narração") { "voz" } else { "rádio" };
        c.set_tags(&p, vec![tag.into()]);
    }
    if std::env::var("KALANGO_AUDIO_DEMO").as_deref() == Ok("rodando") {
        c.toggle_run(0);
    }
    c
}

// ---------------------------------------------------------------- checks of the sound system and the tools

/// How a check came out: teal, gold or red dot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Good,
    Warn,
    Bad,
}

impl Level {
    pub fn token(self) -> &'static str {
        match self {
            Level::Good => "teal",
            Level::Warn => "gold",
            Level::Bad => "red",
        }
    }
}

/// What was found about the sound system and the helper programs (slow to gather: run it in a thread, see
/// [`probe_then`]).
#[derive(Clone, Debug, Default)]
pub struct SoundReport {
    /// forced off by `KALANGO_AUDIO_SEM_SOM=1`
    pub forced_off: bool,
    /// the ALSA library loads (the engine plays and captures through it)
    pub alsa: bool,
    /// the folder that holds PipeWire's socket, when there is one
    pub pipewire_dir: Option<PathBuf>,
    pub pulse_socket: bool,
    /// `pw-cli info 0` answered
    pub pw_answer: bool,
    pub pw_version: Option<String>,
    pub pw_rate: Option<u32>,
    /// (unit, state) from `systemctl --user is-active`
    pub services: Vec<(String, String)>,
    /// helper programs on PATH: pw-record, pw-dump, ffmpeg, ffprobe, gio (transcription: see crate::voz)
    pub tools: Vec<(&'static str, bool)>,
    pub ladspa: usize,
}

pub const TOOLS: [&str; 5] = ["pw-record", "pw-dump", "ffmpeg", "ffprobe", "gio"];

impl SoundReport {
    /// Sound can be played and captured: the ALSA library loads and PipeWire (or its Pulse side) is there.
    pub fn sound_ok(&self) -> bool {
        !self.forced_off && self.alsa && (self.pipewire_dir.is_some() || self.pulse_socket || self.pw_answer)
    }

    pub fn has(&self, tool: &str) -> bool {
        self.tools.iter().any(|(t, ok)| *t == tool && *ok)
    }

    /// The technical lines shown in "Details" (one per thing checked).
    pub fn detail_lines(&self) -> Vec<String> {
        let mut v = vec![];
        if self.forced_off {
            v.push(tr("KALANGO_AUDIO_SEM_SOM=1 is set: the sound system is treated as missing").to_string());
        }
        for (unit, state) in &self.services {
            v.push(format!("{unit}: {state}"));
        }
        v.push(match &self.pipewire_dir {
            Some(d) => trf("PipeWire socket: {path}", &[("path", d.join("pipewire-0").display().to_string())]),
            None => tr("PipeWire socket: not found").to_string(),
        });
        v.push(if self.pw_answer {
            trf("pw-cli info 0: answered (version {v})", &[("v", self.pw_version.clone().unwrap_or_else(|| "?".into()))])
        } else {
            tr("pw-cli info 0: no answer").to_string()
        });
        v.push(if self.alsa { tr("ALSA library (libasound.so.2): loaded").to_string() } else { tr("ALSA library (libasound.so.2): not found").to_string() });
        v
    }
}

/// Where the session's sockets may be: `$XDG_RUNTIME_DIR`, then `/run/user/<uid>`.
pub fn runtime_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        v.push(PathBuf::from(d));
    }
    let uid = unsafe { libc::getuid() };
    let run = PathBuf::from(format!("/run/user/{uid}"));
    if !v.contains(&run) {
        v.push(run);
    }
    v
}

fn pipewire_dir() -> Option<PathBuf> {
    runtime_dirs().into_iter().find(|d| d.join("pipewire-0").exists())
}

fn alsa_loads() -> bool {
    static A: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // the library stays loaded for the app's life anyway (the engine opens it too)
    *A.get_or_init(|| unsafe { libloading::Library::new("libasound.so.2").map(std::mem::forget).is_ok() })
}

fn forced_off() -> bool {
    std::env::var("KALANGO_AUDIO_SEM_SOM").map(|v| v == "1").unwrap_or(false)
}

/// Runs a program and waits at most `secs`: (exit ok, stdout). None when it could not start.
pub fn run_timeout(cmd: &str, args: &[&str], envs: &[(&str, &str)], secs: f64) -> Option<(bool, String)> {
    use std::io::Read;
    let mut c = std::process::Command::new(cmd);
    c.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    for (k, v) in envs {
        c.env(k, v);
    }
    let mut child = c.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                if out.trim().is_empty() {
                    if let Some(mut e) = child.stderr.take() {
                        let _ = e.read_to_string(&mut out);
                    }
                }
                return Some((st.success(), out));
            }
            Ok(None) if start.elapsed().as_secs_f64() < secs => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Some((false, String::new()));
            }
        }
    }
}

/// `version: "1.0.5"` and `default.clock.rate = "48000"` from `pw-cli info 0`.
pub fn parse_pw_info(text: &str) -> (Option<String>, Option<u32>) {
    let value = |l: &str| l.split('"').nth(1).map(str::to_string);
    let mut version = None;
    let mut rate = None;
    for l in text.lines() {
        let t = l.trim_start_matches(['*', ' ', '\t']);
        if version.is_none() && t.starts_with("version:") {
            version = value(t);
        }
        if rate.is_none() && t.starts_with("default.clock.rate") {
            rate = value(t).and_then(|v| v.parse().ok());
        }
    }
    (version, rate)
}

/// Looks at everything (runs `pw-cli`, `systemctl --user`, scans plug-ins): call it off the main thread.
pub fn probe() -> SoundReport {
    let mut r = SoundReport { forced_off: forced_off(), alsa: alsa_loads(), pipewire_dir: pipewire_dir(), ..Default::default() };
    r.pulse_socket = runtime_dirs().iter().any(|d| d.join("pulse/native").exists());
    let dir = r.pipewire_dir.clone().or_else(|| runtime_dirs().into_iter().next()).unwrap_or_default();
    let dir_s = dir.to_string_lossy().to_string();
    if let Some((ok, out)) = run_timeout("pw-cli", &["info", "0"], &[("PIPEWIRE_RUNTIME_DIR", &dir_s)], 2.0) {
        r.pw_answer = ok && out.contains("id: 0");
        if r.pw_answer {
            (r.pw_version, r.pw_rate) = parse_pw_info(&out);
        }
    }
    let units = ["pipewire.service", "pipewire-pulse.service", "wireplumber.service"];
    if let Some((_, out)) = run_timeout("systemctl", &["--user", "is-active", units[0], units[1], units[2]], &[], 2.0) {
        let states: Vec<&str> = out.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if states.len() == units.len() {
            r.services = units.iter().zip(states).map(|(u, s)| (u.to_string(), s.to_string())).collect();
        }
    }
    r.tools = TOOLS.iter().map(|t| (*t, crate::media::have(t))).collect();
    r.ladspa = crate::ladspa::cached().len();
    r
}

/// Runs [`probe`] in a helper thread and calls `f` with the result on the main thread.
pub fn probe_then(f: impl FnOnce(SoundReport) + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(probe());
    });
    let f = RefCell::new(Some(f));
    glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
        Ok(r) => {
            if let Some(f) = f.borrow_mut().take() {
                f(r);
            }
            glib::ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(_) => glib::ControlFlow::Break,
    });
}

/// The quick check (no programs run unless the socket is missing): the ALSA library loads and PipeWire's (or
/// Pulse's) socket is there, or `pw-cli info 0` answers.
pub fn sound_available() -> bool {
    if forced_off() {
        return false;
    }
    if !alsa_loads() {
        return false;
    }
    if pipewire_dir().is_some() || runtime_dirs().iter().any(|d| d.join("pulse/native").exists()) {
        return true;
    }
    run_timeout("pw-cli", &["info", "0"], &[], 1.5).map(|(ok, out)| ok && out.contains("id: 0")).unwrap_or(false)
}

/// Restarts the person's own sound services (a user session needs no administrator for this). Err has what
/// systemctl said.
pub fn restart_sound() -> Result<(), String> {
    match run_timeout("systemctl", &["--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"], &[], 15.0) {
        Some((true, _)) => Ok(()),
        Some((false, out)) => Err(if out.trim().is_empty() { tr("systemctl did not finish").to_string() } else { out.trim().to_string() }),
        None => Err(tr("systemctl was not found").to_string()),
    }
}

/// Opens a KalangoOS app through the shell (`kalango-shell configuracoes privacidade`, `kalango-shell ajuda …`).
/// False when the shell is not there (another desktop).
pub fn open_shell(args: &[&str]) -> bool {
    std::process::Command::new("kalango-shell").args(args).spawn().is_ok()
}

// ---------------------------------------------------------------- first use

/// A session made from a template (by name) with the person's settings, opened in the editor.
fn start_from(ui: &Rc<Ui>, template: &str) {
    let idx = ui.core.borrow().templates.iter().position(|t| t.name == template);
    let Some(t) = idx else { return };
    let i = {
        let mut c = ui.core.borrow_mut();
        c.store.prefs.welcomed = true;
        c.new_with_prefs(t)
    };
    ui.changed(Change::Sessions);
    ui.changed(Change::Store);
    ui.open_session(i);
}

fn logo(px: i32) -> gtk::Image {
    let tex = gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!("../../assets/marca/logo-kalango-audio.png"))).expect("logo");
    // (an Image keeps the size asked; a Picture grows to the texture's 512 px)
    let p = gtk::Image::from_paintable(Some(&tex));
    p.set_pixel_size(px);
    p.set_size_request(px, px);
    p.set_halign(Align::Center);
    p
}

/// One of the three ways to start: chain, title, text, "Start".
fn start_card(ui: &Rc<Ui>, template: &'static str, title: &str, text: &str, recommended: bool) -> gtk::Widget {
    let card = vbox(0, "ka-cartao ka-comeco");
    if recommended {
        card.add_css_class("recomendado");
    }
    card.set_hexpand(true);
    let kinds: Vec<String> = {
        let c = ui.core.borrow();
        c.templates.iter().find(|t| t.name == template).map(|t| ordered_kinds(&t.instantiate(), 3)).unwrap_or_default()
    };
    if kinds.is_empty() {
        let plus = hbox(0, "ka-tracejado ka-comeco-mais");
        plus.set_size_request(36, 36);
        plus.set_halign(Align::Start);
        let i = icons::icon("mais", "muted", 16);
        i.set_hexpand(true);
        plus.append(&i);
        card.append(&plus);
    } else {
        card.append(&chain(&kinds, 36, 14));
    }
    let t = line(title, "ka-titulo2");
    t.set_margin_top(14);
    card.append(&t);
    let d = para(text, "ka-texto2");
    d.set_margin_top(6);
    card.append(&d);
    let b = button(tr("Get started"), if recommended { "primario" } else { "secundario" }, None);
    b.set_halign(Align::Start);
    b.set_margin_top(14);
    let u = ui.clone();
    b.connect_clicked(move |_| start_from(&u, template));
    card.append(&b);
    card.upcast()
}

struct Welcome {
    logo: kalango_movimento::Animavel,
    cards: Vec<kalango_movimento::Animavel>,
    row: gtk::Box,
    checks: gtk::Box,
}

fn fill_checks(ui: &Rc<Ui>, b: &gtk::Box, r: &SoundReport) {
    clear(b);
    b.append(&caps(tr("What was checked")));
    let inputs = {
        let mut c = ui.core.borrow_mut();
        if c.devices.is_empty() {
            c.devices = crate::audio::list_devices();
        }
        c.devices.len().saturating_sub(1)
    };
    let mut rows: Vec<(Level, String)> = vec![];
    rows.push(if r.sound_ok() {
        match &r.pw_version {
            Some(v) => (Level::Good, trf("Sound system (PipeWire {v}) ready", &[("v", v.clone())])),
            None => (Level::Good, tr("Sound system (PipeWire) ready").to_string()),
        }
    } else {
        (Level::Bad, tr("The sound system did not respond").to_string())
    });
    rows.push((
        Level::Warn,
        if inputs > 0 {
            trf("Microphone: {n} sound card(s) found; the system may ask for permission at the first recording", &[("n", inputs.to_string())])
        } else {
            tr("Microphone: the permission is asked at the first recording").to_string()
        },
    ));
    rows.push(if r.has("ffmpeg") {
        (Level::Good, tr("ffmpeg found: MP3, AAC and FLAC available").to_string())
    } else {
        (Level::Warn, tr("ffmpeg not found: recordings in WAV and AIFF only").to_string())
    });
    for (lv, text) in rows {
        let row = hbox(8, "");
        row.append(&dot(lv.token()));
        row.append(&para(&text, "ka-texto2"));
        b.append(&row);
    }
}

pub fn welcome(ui: &Rc<Ui>) -> gtk::Widget {
    let root = vbox(0, "ka-pagina ka-boas-vindas");
    root.set_halign(Align::Center);
    root.set_valign(Align::Center);
    let col = vbox(0, "");
    col.set_size_request(-1, -1);
    root.append(&col);

    let l = logo(220);
    let la = animated(&l);
    la.set_halign(Align::Center);
    col.append(&la);
    let t = label(tr("Welcome to Kalango Audio"), "ka-titulo ka-boas-vindas-titulo");
    t.set_halign(Align::Center);
    t.set_xalign(0.5);
    t.set_margin_top(16);
    col.append(&t);
    let s = para(tr("Capture the sound of any app or microphone, clean it, record and transcribe. Everything runs on your computer."), "ka-texto2");
    s.set_xalign(0.5);
    s.set_justify(gtk::Justification::Center);
    s.set_margin_top(8);
    col.append(&s);

    let row = hbox(16, "ka-comecos");
    row.set_margin_top(32);
    row.set_homogeneous(true);
    let mut cards = vec![];
    for (tpl, title, text, rec) in [
        ("Record an app", tr("Record an app"), tr("Choose an open app and record only its sound."), false),
        ("Online meeting", tr("Online meeting"), tr("Browser + microphone, with the text at the end."), true),
        ("Blank", tr("Start blank"), tr("Build your chain by dragging blocks."), false),
    ] {
        let a = animated(&start_card(ui, tpl, title, text, rec));
        row.append(&a);
        cards.push(a);
    }
    col.append(&row);

    // "Or ask: …"
    let ask = gtk::Overlay::new();
    ask.set_margin_top(32);
    ask.set_halign(Align::Center);
    let e = gtk::Entry::new();
    e.add_css_class("ka-campo");
    e.add_css_class("ka-pedir-grande");
    e.set_placeholder_text(Some(tr("Or ask: “record tomorrow’s class at 8 and transcribe”")));
    e.set_primary_icon_paintable(icons::texture("spark", "accent", 14).as_ref());
    e.set_size_request(660, 44);
    ask.set_child(Some(&e));
    let key = label("Enter", "ka-atalho");
    key.set_halign(Align::End);
    key.set_valign(Align::Center);
    key.set_margin_end(12);
    key.set_can_target(false);
    ask.add_overlay(&key);
    {
        let u = ui.clone();
        e.connect_activate(move |e| {
            let text = e.text().to_string();
            if text.trim().is_empty() {
                return;
            }
            e.set_text("");
            super::agente::ask(&u, &text);
        });
    }
    col.append(&ask);

    let checks = vbox(6, "ka-cartao ka-verificado");
    checks.set_margin_top(32);
    checks.set_size_request(660, -1);
    checks.set_halign(Align::Center);
    checks.append(&caps(tr("What was checked")));
    checks.append(&label(tr("Checking…"), "ka-fraco"));
    col.append(&checks);

    let w = Rc::new(Welcome { logo: la, cards, row: row.clone(), checks: checks.clone() });
    let fit = {
        let (w, e) = (w.clone(), e.clone());
        move |ui: &Rc<Ui>| {
            let narrow = ui.layout() == Layout::Narrow;
            w.row.set_orientation(if narrow { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal });
            w.row.set_size_request(if narrow { 360 } else { 932 }, -1);
            e.set_size_request(if narrow { 360 } else { 660 }, 44);
            w.checks.set_size_request(if narrow { 360 } else { 660 }, -1);
        }
    };
    fit(ui);
    ui.on_change(move |ui, c| {
        if c == Change::Layout {
            fit(ui)
        }
    });
    {
        let w = w.clone();
        ui.on_page(move |ui, name| {
            if name != "boas-vindas" {
                return;
            }
            // the logo rises 12 px with the slow spring, then the cards cascade
            use kalango_movimento::{Canal, LENTA};
            w.logo.definir(Canal::Opacidade, 0.0).definir(Canal::Y, 12.0);
            w.logo.ir(Canal::Opacidade, 1.0, LENTA).ir(Canal::Y, 0.0, LENTA);
            for c in &w.cards {
                c.definir(Canal::Opacidade, 0.0);
            }
            let cards = w.cards.clone();
            glib::timeout_add_local_once(Duration::from_millis(260), move || cascade(&cards));
            let (u, b) = (ui.clone(), w.checks.clone());
            probe_then(move |r| fill_checks(&u, &b, &r));
        });
    }
    scroller(&root).upcast()
}

// ---------------------------------------------------------------- the sound system did not respond

pub fn no_sound(ui: &Rc<Ui>) -> gtk::Widget {
    let root = vbox(0, "ka-pagina ka-sem-som");
    root.set_halign(Align::Center);
    root.set_valign(Align::Center);
    let well = hbox(0, "ka-sem-som-icone");
    well.set_size_request(72, 72);
    well.set_halign(Align::Center);
    let i = icons::icon("alerta", "red", 20);
    i.set_hexpand(true);
    well.append(&i);
    root.append(&well);
    let t = label(tr("The sound system did not respond"), "ka-titulo ka-centro");
    t.set_xalign(0.5);
    t.set_margin_top(28);
    root.append(&t);
    let p = para(
        tr("Kalango Audio uses PipeWire to hear and play sound. The service looks stopped or is not installed, so nothing can be captured right now. Your sessions and recordings are kept."),
        "ka-texto2",
    );
    p.set_xalign(0.5);
    p.set_justify(gtk::Justification::Center);
    p.set_margin_top(10);
    p.set_size_request(580, -1);
    root.append(&p);

    let buttons = hbox(10, "");
    buttons.set_halign(Align::Center);
    buttons.set_margin_top(28);
    let again = button(tr("Try again"), "primario", Some("reiniciar"));
    let restart = button(tr("Restart the sound"), "secundario", Some("escudo"));
    buttons.append(&again);
    buttons.append(&restart);
    root.append(&buttons);
    let note = para(tr("Restarting the sound restarts your own session’s sound services; it needs no administrator password."), "ka-fraco");
    note.set_xalign(0.5);
    note.set_justify(gtk::Justification::Center);
    note.set_margin_top(12);
    root.append(&note);

    let det = vbox(4, "ka-cartao ka-detalhes");
    det.set_margin_top(24);
    det.set_size_request(460, -1);
    det.set_halign(Align::Center);
    det.append(&label(tr("Details"), "ka-texto2 ka-detalhes-titulo"));
    let lines = vbox(2, "");
    lines.append(&label(tr("Checking…"), "ka-fraco"));
    det.append(&lines);
    root.append(&det);

    let help = gtk::Button::with_label(tr("See what to do in the Help and tips app"));
    help.add_css_class("ka-link-botao");
    help.set_halign(Align::Center);
    help.set_margin_top(24);
    {
        let u = ui.clone();
        help.connect_clicked(move |_| {
            if !open_shell(&["ajuda", tr("sound system")]) {
                u.toast(tr("The Help and tips app is part of KalangoOS and was not found here."), None);
            }
        });
    }
    root.append(&help);

    let fill = {
        let lines = lines.clone();
        move |r: &SoundReport| {
            clear(&lines);
            for l in r.detail_lines() {
                let x = label(&l, "ka-mono ka-fraco");
                x.set_wrap(true);
                x.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                lines.append(&x);
            }
        }
    };
    let fill = Rc::new(fill);
    let busy = Rc::new(Cell::new(false));
    // "Try again": check; back to the sessions when sound answers
    let retry = {
        let (fill, busy, again, ui) = (fill.clone(), busy.clone(), again.clone(), Rc::downgrade(ui));
        Rc::new(move || {
            if busy.replace(true) {
                return;
            }
            again.set_sensitive(false);
            let (fill, busy, again, ui) = (fill.clone(), busy.clone(), again.clone(), ui.clone());
            probe_then(move |r| {
                busy.set(false);
                again.set_sensitive(true);
                fill(&r);
                let Some(ui) = ui.upgrade() else { return };
                if r.sound_ok() {
                    leave_no_sound(&ui);
                } else if ui.page() == "sem-som" {
                    ui.toast(tr("Still no answer from the sound system."), None);
                }
            });
        })
    };
    {
        let r = retry.clone();
        again.connect_clicked(move |_| r());
    }
    {
        let (u, r) = (ui.clone(), retry.clone());
        let note = note.clone();
        restart.connect_clicked(move |b| {
            b.set_sensitive(false);
            note.set_text(tr("Restarting the sound services…"));
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(restart_sound());
            });
            let (b, u, r, note) = (b.clone(), u.clone(), r.clone(), note.clone());
            glib::timeout_add_local(Duration::from_millis(80), move || match rx.try_recv() {
                Ok(res) => {
                    b.set_sensitive(true);
                    match res {
                        Ok(()) => {
                            note.set_text(tr("The sound services were restarted."));
                            // give them a moment to come up
                            let r = r.clone();
                            glib::timeout_add_local_once(Duration::from_millis(1200), move || r());
                        }
                        Err(e) => {
                            note.set_text(&trf("Could not restart the sound: {err}", &[("err", e.clone())]));
                            u.toast(tr("Restarting the sound did not work; the details are on the page."), None);
                        }
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => glib::ControlFlow::Break,
            });
        });
    }
    {
        let fill = fill.clone();
        ui.on_page(move |_, name| {
            if name == "sem-som" {
                let fill = fill.clone();
                probe_then(move |r| fill(&r));
            }
        });
    }
    scroller(&root).upcast()
}

thread_local! {
    /// the page that was showing when sound went away (to go back to it)
    static BEFORE: RefCell<String> = const { RefCell::new(String::new()) };
}

fn leave_no_sound(ui: &Rc<Ui>) {
    if ui.page() != "sem-som" {
        return;
    }
    let back = BEFORE.with(|b| b.take());
    let back = if back.is_empty() || back == "sem-som" { "sessoes".to_string() } else { back };
    ui.go(&back);
    ui.toast(tr("The sound system is back."), None);
}

/// Live checks while the app runs: every 5 s the sound system (switching to and from "sem-som"); the recorders'
/// errors (disk full and others) as toasts; the agent's panel when `KALANGO_AUDIO_PEDIDO` asks for it at start.
pub fn start_hooks(ui: &Rc<Ui>) {
    // the sound system, in a helper thread so the window never waits
    {
        let busy = Rc::new(Cell::new(false));
        let w = Rc::downgrade(ui);
        glib::timeout_add_seconds_local(5, move || {
            let Some(ui) = w.upgrade() else { return glib::ControlFlow::Break };
            if busy.replace(true) {
                return glib::ControlFlow::Continue;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(sound_available());
            });
            let (busy, w) = (busy.clone(), Rc::downgrade(&ui));
            glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
                Ok(ok) => {
                    busy.set(false);
                    if let Some(ui) = w.upgrade() {
                        let page = ui.page();
                        if !ok && page != "sem-som" {
                            BEFORE.with(|b| *b.borrow_mut() = page);
                            ui.go("sem-som");
                        } else if ok && page == "sem-som" {
                            leave_no_sound(&ui);
                        }
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => {
                    busy.set(false);
                    glib::ControlFlow::Break
                }
            });
            glib::ControlFlow::Continue
        });
    }
    // recorder errors: one toast per new error state
    {
        let seen: RefCell<std::collections::HashSet<(u64, u64, String)>> = RefCell::default();
        let n = Cell::new(0u32);
        ui.on_tick(move |ui| {
            n.set(n.get() + 1);
            if !n.get().is_multiple_of(15) {
                return;
            }
            let found: Vec<(u64, u64, String, String, String)> = {
                let c = ui.core.borrow();
                let mut v = vec![];
                for r in &c.running {
                    let Some(s) = c.sessions.iter().find(|s| s.uid == r.uid) else { continue };
                    for b in s.blocks.iter().filter(|b| b.kind == "recorder") {
                        if let Some(st) = c.rec_status(s.uid, b.id) {
                            if st.state.starts_with("error") {
                                v.push((s.uid, b.id, st.state.clone(), s.name.clone(), b.p.text("folder", &c.store.prefs.default_folder)));
                            }
                        }
                    }
                }
                v
            };
            for (uid, id, state, name, folder) in found {
                if !seen.borrow_mut().insert((uid, id, state.clone())) {
                    continue;
                }
                let what = state.trim_start_matches("error").trim_start_matches(':').trim().to_string();
                if is_disk_full(&what) {
                    let dir = crate::core::expand_home(&folder);
                    ui.toast(
                        &trf("The recording stopped: disk full. What was recorded until then is saved in {folder}.", &[("folder", folder.clone())]),
                        Some((
                            tr("Free up space"),
                            Box::new(move |_| {
                                let _ = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(&dir).uri(), None::<&gio::AppLaunchContext>);
                            }),
                        )),
                    );
                } else {
                    ui.toast(&trf("“{name}” stopped recording: {err}", &[("name", name), ("err", what)]), None);
                }
            }
        });
    }
    // KALANGO_AUDIO_PEDIDO="grave a reunião do navegador e transcreva" opens the agent's panel at start
    if let Ok(text) = std::env::var("KALANGO_AUDIO_PEDIDO") {
        let u = ui.clone();
        glib::timeout_add_local_once(Duration::from_millis(300), move || super::agente::ask(&u, &text));
    }
}

/// An error message that means the disk is full (the system's English or Portuguese text, or errno 28).
pub fn is_disk_full(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("os error 28") || m.contains("no space left") || m.contains("espaço insuficiente") || m.contains("disk full")
}

// ---------------------------------------------------------------- dialogs

/// A dialog over the pages: veil + centred box; returns (box, close).
pub fn dialog(ui: &Rc<Ui>, width: i32) -> (gtk::Box, Rc<dyn Fn()>) {
    let veil = gtk::Box::new(gtk::Orientation::Vertical, 0);
    veil.add_css_class("ka-veu");
    veil.set_hexpand(true);
    veil.set_vexpand(true);
    let d = vbox(0, "ka-dialogo");
    d.set_size_request(width, -1);
    d.set_halign(Align::Center);
    d.set_valign(Align::Center);
    let anim = animated(&d);
    anim.set_halign(Align::Center);
    anim.set_valign(Align::Center);
    anim.set_hexpand(true);
    anim.set_vexpand(true);
    veil.append(&anim);
    ui.overlay.add_overlay(&veil);
    anim.entrar((0.0, 8.0), 0.98);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let done = Rc::new(Cell::new(false));
    let close: Rc<dyn Fn()> = {
        let (ov, veil, anim, win, keys, done) = (ui.overlay.clone(), veil.clone(), anim.clone(), ui.window.clone(), keys.clone(), done.clone());
        Rc::new(move || {
            if done.replace(true) {
                return;
            }
            win.remove_controller(&keys);
            let (ov, veil) = (ov.clone(), veil.clone());
            anim.sair((0.0, 8.0), 0.98, move || ov.remove_overlay(&veil));
        })
    };
    {
        let c = close.clone();
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Escape {
                c();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    ui.window.add_controller(keys);
    // a click on the veil (outside the box) closes
    let click = gtk::GestureClick::new();
    {
        let (c, d) = (close.clone(), d.clone());
        click.connect_pressed(move |g, _, x, y| {
            let Some(w) = g.widget() else { return };
            if let Some(p) = w.compute_point(&d, &gtk::graphene::Point::new(x as f32, y as f32)) {
                if p.x() < 0.0 || p.y() < 0.0 || p.x() > d.width() as f32 || p.y() > d.height() as f32 {
                    c();
                }
            }
        });
    }
    veil.add_controller(click);
    (d, close)
}

pub fn confirm_quit(ui: &Rc<Ui>) {
    let (d, close) = dialog(ui, 420);
    let n = ui.core.borrow().recordings_running();
    d.append(&label(tr("Stop the recordings and quit?"), "ka-titulo2"));
    let p = para(
        &trf("{n} recording(s) still running. They will be stopped and their files closed properly before the app quits.", &[("n", n.to_string())]),
        "ka-texto2",
    );
    p.set_margin_top(8);
    d.append(&p);
    let row = hbox(8, "");
    row.set_margin_top(20);
    row.set_halign(Align::End);
    let cancel = button(tr("Cancel"), "fantasma", None);
    let quit = button(tr("Stop and quit"), "perigo", None);
    row.append(&cancel);
    row.append(&quit);
    d.append(&row);
    {
        let (u, c) = (ui.clone(), close.clone());
        cancel.connect_clicked(move |_| {
            u.core.borrow_mut().confirm_quit(false);
            c();
        });
    }
    {
        let (u, c) = (ui.clone(), close.clone());
        quit.connect_clicked(move |_| {
            c();
            u.core.borrow_mut().confirm_quit(true);
            u.changed(Change::Running);
            // the main loop sees quit_now, saves and quits
        });
    }
    quit.grab_focus();
}

fn json_filter() -> gio::ListStore {
    let f = gtk::FileFilter::new();
    f.set_name(Some(tr("Kalango Audio sessions (JSON)")));
    f.add_pattern("*.json");
    f.add_mime_type("application/json");
    let l = gio::ListStore::new::<gtk::FileFilter>();
    l.append(&f);
    l
}

pub fn import_dialog(ui: &Rc<Ui>) {
    let fd = gtk::FileDialog::builder().title(tr("Import session")).modal(true).filters(&json_filter()).build();
    let u = ui.clone();
    fd.open(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        let r = u.core.borrow_mut().import_session(&path.to_string_lossy());
        match r {
            Ok(msg) => {
                u.changed(Change::Sessions);
                let i = u.core.borrow().cur;
                u.toast(&msg, Some((tr("Open"), Box::new(move |u: &Rc<Ui>| u.open_session(i)))));
            }
            Err(e) => u.toast(&e, None),
        }
    });
}

pub fn export_dialog(ui: &Rc<Ui>) {
    let name = ui.core.borrow().session().map(|s| s.name.clone());
    let Some(name) = name else {
        ui.toast(tr("No session is open"), None);
        return;
    };
    let fd = gtk::FileDialog::builder().title(tr("Export session")).modal(true).filters(&json_filter()).initial_name(format!("{name}.json")).build();
    let u = ui.clone();
    fd.save(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        let r = u.core.borrow_mut().export_session(&path.to_string_lossy());
        match r {
            Ok(()) => u.toast(&trf("Session exported to {path}", &[("path", path.display().to_string())]), None),
            Err(e) => u.toast(&trf("Could not export: {err}", &[("err", e)]), None),
        }
    });
}

pub fn about(ui: &Rc<Ui>) {
    let (d, close) = dialog(ui, 400);
    let top = hbox(14, "");
    let icon = gtk::Image::from_paintable(Some(
        &gdk::Texture::from_bytes(&glib::Bytes::from_static(include_bytes!("../../assets/icons/hicolor/128x128/apps/os.kalango.Audio.png"))).expect("app icon"),
    ));
    icon.set_pixel_size(56);
    top.append(&icon);
    let names = vbox(4, "");
    names.set_valign(Align::Center);
    names.append(&label(&format!("Kalango Audio {}", env!("CARGO_PKG_VERSION")), "ka-titulo2"));
    names.append(&para(tr("Part of KalangoOS. Licence GPL-3.0-or-later."), "ka-texto2"));
    top.append(&names);
    d.append(&top);
    let link = gtk::LinkButton::with_label(env!("CARGO_PKG_REPOSITORY"), tr("Source code"));
    link.add_css_class("ka-link-botao");
    link.set_halign(Align::Start);
    link.set_margin_top(14);
    d.append(&link);
    let keys = para(tr("Shortcuts: Ctrl+N new session · Ctrl+K search or ask · Ctrl+R start/stop · Ctrl+Z / Ctrl+Shift+Z undo/redo · Ctrl+, settings · Ctrl+Q quit"), "ka-fraco");
    keys.set_margin_top(12);
    d.append(&keys);
    let ok = button(tr("Close"), "secundario", None);
    ok.set_halign(Align::End);
    ok.set_margin_top(18);
    let c = close.clone();
    ok.connect_clicked(move |_| c());
    d.append(&ok);
}

/// Opens the system's privacy settings on a section ("microfone", "captura"); false when the shell is not there.
pub fn open_privacy() -> bool {
    open_shell(&["configuracoes", "privacidade"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pw_info_is_read() {
        let text = "\tid: 0\n\tversion: \"1.0.5\"\n\tname: \"pipewire-0\"\n*\tproperties:\n*\t\tdefault.clock.rate = \"48000\"\n";
        assert_eq!(parse_pw_info(text), (Some("1.0.5".into()), Some(48000)));
        assert_eq!(parse_pw_info(""), (None, None));
    }

    #[test]
    fn disk_full_is_recognised() {
        assert!(is_disk_full("No space left on device (os error 28)"));
        assert!(is_disk_full("Espaço insuficiente no dispositivo (os error 28)"));
        assert!(!is_disk_full("could not open “hw:0”"));
    }

    #[test]
    fn the_override_turns_sound_off() {
        let r = SoundReport { forced_off: true, alsa: true, pulse_socket: true, ..Default::default() };
        assert!(!r.sound_ok());
        let r = SoundReport { alsa: true, pulse_socket: true, ..Default::default() };
        assert!(r.sound_ok());
        let r = SoundReport { alsa: false, pulse_socket: true, ..Default::default() };
        assert!(!r.sound_ok(), "without ALSA nothing plays");
    }

    #[test]
    fn slow_programs_are_cut_off() {
        let t = Instant::now();
        let r = run_timeout("sleep", &["5"], &[], 0.3);
        assert_eq!(r.map(|x| x.0), Some(false));
        assert!(t.elapsed().as_secs_f64() < 2.0);
        assert!(run_timeout("this-program-does-not-exist-kalango", &[], &[], 1.0).is_none());
    }
}
