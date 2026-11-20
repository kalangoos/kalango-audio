//! The other screens of the app: menu bar, sessions window, recordings window, presets,
//! scripts (JavaScript), preferences, mini panel, new-session chooser, import/export, help,
//! and the side tabs (blocks, settings, recordings, schedule, automations, info).
use super::*;
use crate::media::{self, Player, Tags};
use crate::script;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Prefs {
    pub audio_editor: String,
    pub default_folder: String,
    #[serde(default)]
    pub bar_dismissed: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Preset {
    pub kind: String,
    pub name: String,
    pub p: Params,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Script {
    pub name: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Store {
    pub prefs: Prefs,
    pub presets: Vec<Preset>,
    pub scripts: Vec<Script>,
}

#[derive(Clone)]
pub struct RecFile {
    path: PathBuf,
    name: String,
    folder: String,
    size: u64,
    modified: u64,
    dur: Option<f64>,
}

/// Something that happened that an automation can react to.
#[derive(Clone)]
pub struct ScriptEvent {
    pub name: String,
    pub session: Option<u64>,
    pub file: Option<script::RecordingInfo>,
}

#[derive(Default)]
struct Insp {
    for_path: Option<PathBuf>,
    tags: Tags,
    name_edit: String,
    dur: Option<f64>,
    msg: String,
    trash_confirm: bool,
    tags_open: bool,
}

pub struct Extras {
    pub store: Store,
    saved: String,
    last_save: Instant,
    pub right_tab: usize,
    pub blocks_filter: String,
    pub prev_sel: Option<u64>,
    pub show_recordings: bool,
    pub show_presets: bool,
    pub show_scripts: bool,
    pub show_mini: bool,
    pub show_prefs: bool,
    pub show_about: bool,
    pub show_io: bool,
    pub show_home: bool,
    pub show_script_help: bool,
    /// a line shown at the top of the Preferences window (e.g. why it was opened)
    pub prefs_note: String,
    recs: Vec<RecFile>,
    rec_scan: Option<Instant>,
    rec_sel: Option<usize>,
    insp: Insp,
    player: Option<(PathBuf, Player)>,
    pub io_path: String,
    io_msg: String,
    preset_name: String,
    /// (is built-in, index)
    script_sel: (bool, usize),
    log: Vec<String>,
    dialogs: Vec<String>,
    queue: VecDeque<ScriptEvent>,
    tpl_sel: usize,
    pub timer_state: HashMap<(u64, usize), bool>,
    pub launched: bool,
    /// 0 = normal, 1 = asking, 2 = finishing recordings before quitting
    pub quit_state: u8,
    pub quit_now: bool,
    pub closing: bool,
    last_dev: Instant,
    undo: Vec<(u64, String)>,
    redo: Vec<(u64, String)>,
    base: Option<(u64, String)>,
    last_seen: String,
    last_change: Instant,
    home_cols: [bool; 5],
    home_sort: (usize, bool),
    home_sel: Option<usize>,
}

fn store_path() -> PathBuf {
    config_path().with_file_name("extras.json")
}

impl Extras {
    pub fn new(shot: bool) -> Self {
        let mut store: Store = if shot { Store::default() } else {
            std::fs::read_to_string(store_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
        };
        if store.prefs.default_folder.is_empty() { store.prefs.default_folder = engine::default_recordings_dir(); }
        let saved = serde_json::to_string(&store).unwrap_or_default();
        Extras {
            store, saved, last_save: Instant::now(), right_tab: 0, blocks_filter: String::new(), prev_sel: None,
            show_recordings: false, show_presets: false, show_scripts: false, show_mini: false, show_prefs: false, show_about: false,
            show_io: false, show_home: false, show_script_help: false, prefs_note: String::new(),
            recs: vec![], rec_scan: None, rec_sel: None, insp: Insp::default(), player: None,
            io_path: String::new(), io_msg: String::new(), preset_name: String::new(),
            script_sel: (true, 0), log: vec![], dialogs: vec![], queue: VecDeque::new(), tpl_sel: 0,
            timer_state: HashMap::new(), launched: shot, quit_state: 0, quit_now: false, closing: false, last_dev: Instant::now(),
            undo: vec![], redo: vec![], base: None, last_seen: String::new(), last_change: Instant::now(),
            home_cols: [true; 5], home_sort: (0, true), home_sel: None,
        }
    }

    pub fn queue_event(&mut self, name: &str, session: Option<u64>, file: Option<script::RecordingInfo>) {
        self.queue.push_back(ScriptEvent { name: name.to_string(), session, file });
    }

    /// Files that a session has finished writing become "recording finished" events.
    pub fn on_files_ended(&mut self, uid: u64, ends: Vec<engine::FileEnd>) {
        for f in ends {
            let name = std::path::Path::new(&f.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.queue_event("recording_finished", Some(uid), Some(script::RecordingInfo { path: f.path, name, seconds: f.seconds, bytes: f.size }));
        }
    }
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(r) = p.strip_prefix('~') {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(r.trim_start_matches('/'));
    }
    PathBuf::from(p)
}

fn fmt_epoch(e: u64) -> String {
    unsafe {
        let t = e as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
    }
}

fn now_hms() -> String {
    let (_, m) = engine::local_now();
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() % 60).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", m / 60, m % 60, s)
}

fn tool_on_path(name: &str) -> bool { media::have(name) }

fn fmt_clock(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Our own equalizer presets, written as dB per band (31 Hz .. 16 kHz). The sliders store
/// 0..1 with 0.5 = flat over a +-12 dB range, so each value becomes 0.5 + dB / 24.
pub const EQ_PRESETS_DB: [(&str, [f64; 10]); 12] = [
    ("Plano", [0.0; 10]),
    ("Voz clara", [-6.0, -4.0, -2.0, 0.0, 1.0, 2.0, 3.0, 3.0, 2.0, 0.0]),
    ("Podcast quente", [-4.0, 0.0, 2.0, 1.0, 0.0, -1.0, 1.0, 2.0, 1.0, 0.0]),
    ("Mais graves", [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Menos graves", [-6.0, -5.0, -4.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Mais agudos", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 4.0, 5.0, 6.0]),
    ("Menos agudos", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -2.0, -4.0, -5.0, -6.0]),
    ("Fones", [3.0, 2.0, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 1.0]),
    ("Alto-falante de notebook", [-8.0, -6.0, -3.0, 0.0, 2.0, 2.0, 2.0, 1.0, 0.0, -1.0]),
    ("Rádio antigo", [-12.0, -12.0, -6.0, 0.0, 3.0, 4.0, 2.0, -6.0, -12.0, -12.0]),
    ("Música ao vivo", [2.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0, 2.0, 2.0, 1.0]),
    ("Volume baixo à noite", [5.0, 4.0, 2.0, 0.0, -1.0, -1.0, 0.0, 2.0, 3.0, 3.0]),
];

pub fn eq_presets() -> Vec<(String, [f64; 10])> {
    EQ_PRESETS_DB.iter().map(|(n, db)| { let mut a = [0.5; 10]; for (i, x) in db.iter().enumerate() { a[i] = (0.5 + x / 24.0).clamp(0.0, 1.0); } (n.to_string(), a) }).collect()
}

/// Our own compressor modes: (name, amount 0..1, speed 0..1, make-up gain).
pub const COMPRESSOR_MODES: [(&str, f64, f64, f64); 5] = [
    ("Desligado", 0.0, 0.0, 1.0),
    ("Fala", 0.75, 0.6, 1.2),
    ("Música", 0.5, 0.3, 1.1),
    ("Transmissão", 0.9, 0.8, 1.3),
    ("Filme", 0.6, 0.4, 1.15),
];

pub fn builtin_presets() -> Vec<Preset> {
    let mut v = vec![];
    for (fmt, br, name) in [
        ("AAC", 64, "AAC · voz (64 kbps)"), ("MP3", 96, "MP3 · voz (96 kbps)"), ("MP3", 192, "MP3 · música (192 kbps)"), ("MP3", 320, "MP3 · máximo (320 kbps)"),
        ("AAC", 160, "AAC · música (160 kbps)"), ("FLAC", 0, "FLAC · sem perdas"), ("WAV", 0, "WAV · sem compressão"),
    ] {
        let mut p = Params::default();
        p.set_text("format", fmt);
        if br > 0 { p.set("bitrate", br as f64); }
        v.push(Preset { kind: "recorder".into(), name: name.into(), p });
    }
    for (name, vals) in eq_presets() {
        let mut p = Params::default();
        for (i, x) in vals.iter().enumerate() { p.set(&format!("band{i}"), *x); }
        v.push(Preset { kind: "graphic_eq".into(), name, p });
    }
    for (name, push, speed, makeup) in COMPRESSOR_MODES {
        let mut p = Params::default();
        p.set("amount", push); p.set("speed", speed); p.set("makeup", makeup);
        v.push(Preset { kind: "compressor".into(), name: name.into(), p });
    }
    v
}

fn apply_preset(b: &mut Block, p: &Preset) {
    for (k, v) in &p.p.n { b.p.n.insert(k.clone(), *v); }
    for (k, v) in &p.p.s { b.p.s.insert(k.clone(), v.clone()); }
}

/// What a timer says about "now": is its window open?
pub fn timer_open(t: &Timer, wd: u32, min: u32, today: u32) -> bool {
    if !t.on { return false; }
    let end = t.start as u32 + t.minutes as u32;
    if t.mode == 0 {
        // On Date: one day only (a window that runs past midnight is not carried into the next day)
        return t.date == today && min >= t.start as u32 && min < end;
    }
    let in_today = t.days & (1 << wd) != 0 && min >= t.start as u32 && min < end;
    // a window that runs past midnight is still open after midnight, on the next day
    let prev = (wd + 6) % 7;
    let spill = t.days & (1 << prev) != 0 && end > 1440 && min < end - 1440;
    in_today || spill
}

impl App {
    // ---------------- helpers ----------------

    pub fn find_session(&self, name: &str) -> Option<usize> {
        self.sessions.iter().position(|s| s.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn duplicate_session(&mut self) {
        if self.sessions.is_empty() { return; }
        let mut s = self.sessions[self.cur].clone();
        s.uid = new_uid();
        s.name = format!("{} (cópia)", s.name);
        self.sessions.insert(self.cur + 1, s);
        self.cur += 1;
        self.sel = None;
    }

    pub fn delete_session(&mut self) {
        if self.sessions.is_empty() { return; }
        let uid = self.sessions[self.cur].uid;
        self.running.retain(|r| r.uid != uid);
        self.sessions.remove(self.cur);
        self.sel = None;
    }

    /// Removes a block; in automatic mode the blocks around it are joined together.
    pub fn remove_block_smart(&mut self, id: u64) {
        let s = &mut self.sessions[self.cur];
        if !s.manual {
            let outs = s.block(id).map(|b| b.outs.clone()).unwrap_or_default();
            let ups: Vec<u64> = s.blocks.iter().filter(|b| b.outs.contains(&id)).map(|b| b.id).collect();
            for u in ups { for o in &outs { s.connect(u, *o); } }
        }
        s.remove_block(id);
        if self.sel == Some(id) { self.sel = None; }
    }

    pub fn add_from_library(&mut self, kind: &'static str) {
        let default_folder = self.x.store.prefs.default_folder.clone();
        let manual = self.sessions[self.cur].manual;
        let sel = self.sel;
        let s = &mut self.sessions[self.cur];
        let after = sel.and_then(|i| s.block(i)).map(|b| (b.id, b.x, b.y, b.outs.clone()));
        let (x, y) = match (&after, manual, is_source(kind)) {
            (Some((_, bx, by, _)), false, false) => (*bx + 6.0, *by),
            _ => {
                let (mut x, mut y) = (2.0, 2.0);
                while s.blocks.iter().any(|b| (b.x - x).abs() < 4.0 && (b.y - y).abs() < 3.5) { x += 6.0; if x > 30.0 { x = 2.0; y += 5.0; } }
                (x, y)
            }
        };
        let id = s.add_block(kind, x, y);
        if kind == "recorder" {
            if let Some(b) = s.blocks.iter_mut().find(|b| b.id == id) { b.p.set_text("folder", &default_folder); }
        }
        // automatic mode: the new block goes right after the selected one
        if let (Some((aid, _, _, outs)), false, false) = (after, manual, is_source(kind)) {
            let a_is_end = matches!(s.block(aid).map(|b| b.kind.as_str()), Some("device_output") | Some("radio_stream") | Some("rtmp_stream"));
            if !a_is_end {
                for o in &outs { s.disconnect(aid, *o); s.connect(id, *o); }
                s.connect(aid, id);
            }
        }
        self.sel = Some(id);
        self.x.right_tab = 1;
    }

    pub fn export_session(&mut self, path: &str) -> Result<(), String> {
        let s = self.sessions.get(self.cur).ok_or("nenhuma sessão aberta")?;
        let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
        let p = expand_home(path);
        if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
        std::fs::write(&p, text).map_err(|e| e.to_string())
    }

    pub fn import_session(&mut self, path: &str) -> Result<String, String> {
        let p = expand_home(path);
        let text = std::fs::read(&p).map_err(|e| format!("não foi possível ler o arquivo: {e}"))?;
        let (mut s, note) = (serde_json::from_slice::<Session>(&text).map_err(|e| format!("não é uma sessão do Kalango Audio: {e}"))?, String::new());
        s.uid = new_uid();
        for a in s.automations.iter_mut() { a.migrate(); }
        let name = s.name.clone();
        self.sessions.push(s);
        self.cur = self.sessions.len() - 1;
        self.sel = None;
        Ok(format!("Sessão “{name}” importada{note}"))
    }

    // ---------------- undo and redo ----------------

    /// Watches the open session; when its settings stop changing for half a second that state becomes one undo step.
    pub fn track_undo(&mut self) {
        let Some(s) = self.sessions.get(self.cur) else { return };
        let (uid, snap) = (s.uid, serde_json::to_string(s).unwrap_or_default());
        match &self.x.base {
            Some((u, _)) if *u == uid => {}
            _ => { self.x.base = Some((uid, snap.clone())); self.x.last_seen = snap.clone(); return; }
        }
        if snap != self.x.last_seen { self.x.last_seen = snap.clone(); self.x.last_change = Instant::now(); }
        if self.x.base.as_ref().map(|b| b.1 != snap).unwrap_or(false) && self.x.last_change.elapsed().as_millis() > 500 { self.commit_undo(snap); }
    }

    fn commit_undo(&mut self, snap: String) {
        if let Some(b) = self.x.base.take() {
            self.x.undo.push(b);
            if self.x.undo.len() > 100 { self.x.undo.remove(0); }
            self.x.redo.clear();
            self.x.base = Some((self.sessions[self.cur].uid, snap));
        }
    }

    fn settle_undo(&mut self) {
        if let Some(s) = self.sessions.get(self.cur) {
            let snap = serde_json::to_string(s).unwrap_or_default();
            if self.x.base.as_ref().map(|b| b.0 == s.uid && b.1 != snap).unwrap_or(false) { self.commit_undo(snap); }
        }
    }

    fn restore(&mut self, uid: u64, state: &str) {
        let Some(i) = self.sessions.iter().position(|s| s.uid == uid) else { return };
        if let Ok(mut s) = serde_json::from_str::<Session>(state) {
            s.uid = uid;
            self.sessions[i] = s;
            self.cur = i;
            self.sel = None;
            self.x.base = Some((uid, state.to_string()));
            self.x.last_seen = state.to_string();
        }
    }

    pub fn undo(&mut self) {
        self.settle_undo();
        if let (Some((uid, state)), Some(cur)) = (self.x.undo.pop(), self.x.base.clone()) {
            self.x.redo.push(cur);
            self.restore(uid, &state);
        }
    }

    pub fn redo(&mut self) {
        self.settle_undo();
        if let (Some((uid, state)), Some(cur)) = (self.x.redo.pop(), self.x.base.clone()) {
            self.x.undo.push(cur);
            self.restore(uid, &state);
        }
    }

    // ---------------- scripts ----------------

    fn script_text(&self, name: &str) -> Option<String> {
        script::BUILTIN.iter().find(|(n, _)| *n == name).map(|(_, t)| t.to_string())
            .or_else(|| self.x.store.scripts.iter().find(|s| s.name == name).map(|s| s.text.clone()))
    }

    fn script_input(&self, ev: &ScriptEvent) -> script::Input {
        let sessions = self.sessions.iter().map(|s| script::SessionInfo {
            name: s.name.clone(),
            running: self.is_running(s.uid),
            blocks: s.blocks.iter().map(|b| script::BlockInfo { id: b.id, title: kind_info(&b.kind).map(|k| k.title.to_string()).unwrap_or_default(), bypassed: !b.enabled }).collect(),
        }).collect();
        script::Input {
            sessions,
            kind: ev.name.clone(),
            session: ev.session.and_then(|u| self.sessions.iter().find(|s| s.uid == u)).map(|s| s.name.clone()),
            recording: ev.file.clone(),
        }
    }

    /// Runs a script by name for an event and carries out what it asked for.
    pub fn run_script(&mut self, name: &str, ev: &ScriptEvent) {
        let Some(text) = self.script_text(name) else { self.x.log.push(format!("{}  nenhum script chamado “{name}”", now_hms())); return };
        let out = script::run(&text, &self.script_input(ev));
        let stamp = now_hms();
        if let Some(e) = &out.err { self.x.log.push(format!("{stamp}  “{name}” parou com um erro: {e}")); }
        for a in &out.acts {
            let arg = a.get(1).cloned().unwrap_or_default();
            match a[0].as_str() {
                "log" => self.x.log.push(format!("{stamp}  {arg}")),
                "notify" => { self.x.log.push(format!("{stamp}  {arg}")); if !script::desktop_notify(&arg) { self.x.dialogs.push(arg); } }
                "start_session" | "stop_session" => match self.find_session(&arg) {
                    Some(i) => {
                        let want = a[0] == "start_session";
                        if want != self.is_running(self.sessions[i].uid) { self.toggle_run(i); }
                    }
                    None => self.x.log.push(format!("{stamp}  nenhuma sessão chamada “{arg}”")),
                },
                "stop_all" => { for i in 0..self.sessions.len() { if self.is_running(self.sessions[i].uid) { self.toggle_run(i); } } }
                "quit" => { self.x.quit_now = true; }
                "focus" => script::request_focus(),
                "open_session" => { if let Some(i) = self.find_session(&arg) { self.cur = i; self.sel = None; } }
                "trash" => { if !media::trash(std::path::Path::new(&arg)) { self.x.log.push(format!("{stamp}  não foi possível mover “{arg}” para a lixeira")); } self.x.rec_scan = None; }
                "show_in_files" => { if let Some(d) = std::path::Path::new(&arg).parent() { let _ = std::process::Command::new("xdg-open").arg(d).spawn(); } }
                "open_player" => { let _ = std::process::Command::new("xdg-open").arg(&arg).spawn(); }
                "transcribe" => {
                    // model / language from the script, else from a Transcribe block (the event's session first)
                    let blocks: Vec<&Block> = ev.session.and_then(|u| self.sessions.iter().find(|s| s.uid == u)).into_iter().chain(self.sessions.iter()).flat_map(|s| s.blocks.iter()).filter(|b| b.kind == "transcribe").collect();
                    let pick = |k: &str, given: Option<&String>| given.filter(|v| !v.is_empty()).cloned().or_else(|| blocks.iter().map(|b| b.p.text(k, "")).find(|v| !v.is_empty())).unwrap_or_default();
                    let (model, language, command) = (pick("model", a.get(2)), pick("language", a.get(3)), pick("command", None));
                    match script::transcribe_file(&arg, &command, &model, &language) {
                        Ok(txt) => self.x.log.push(format!("{stamp}  transcrevendo para “{}”", txt.display())),
                        Err(e) => self.x.log.push(format!("{stamp}  não deu para transcrever: {e}")),
                    }
                }
                "open_editor" => {
                    let ed = self.x.store.prefs.audio_editor.trim().to_string();
                    if ed.is_empty() { self.ask_for_editor(); } else { let _ = std::process::Command::new(ed).arg(&arg).spawn(); }
                }
                _ => {}
            }
        }
        for (sname, id, disabled) in &out.blocks {
            if let Some(i) = self.find_session(sname) {
                if let Some(b) = self.sessions[i].blocks.iter_mut().find(|b| b.id == *id) { b.enabled = !*disabled; }
            }
        }
    }

    /// "Rodar agora": scripts that need a file or a session get the latest recording / the open session.
    fn run_script_now(&mut self, name: &str) {
        let text = self.script_text(name).unwrap_or_default();
        let uid = self.sessions.get(self.cur).map(|s| s.uid);
        let needs = script::needs(&text);
        let mut ev = ScriptEvent { name: "manual".into(), session: if needs.session { uid } else { None }, file: None };
        if needs.recording {
            if self.x.rec_scan.is_none() { self.scan_recordings(); }
            if let Some(r) = self.x.recs.first() {
                ev.file = Some(script::RecordingInfo { path: r.path.to_string_lossy().to_string(), name: r.name.clone(), seconds: r.dur.or_else(|| media::duration(&r.path)).unwrap_or(0.0), bytes: r.size });
            } else { self.x.log.push(format!("{}  “{name}” precisa de uma gravação, e ainda não há nenhuma", now_hms())); return; }
        }
        self.run_script(name, &ev);
    }

    fn fire_events(&mut self) {
        let mut guard = 0;
        while let Some(ev) = self.x.queue.pop_front() {
            guard += 1;
            if guard > 100 { self.x.queue.clear(); break; }
            let mut todo: Vec<String> = vec![];
            for s in &self.sessions {
                if ev.session.is_some() && ev.session != Some(s.uid) { continue; }
                for a in &s.automations { if a.on && a.event == ev.name && !a.script.is_empty() { todo.push(a.script.clone()); } }
            }
            for name in todo {
                self.x.log.push(format!("{}  {} → executando “{name}”", now_hms(), EVENTS.iter().find(|e| e.0 == ev.name).map(|e| e.1).unwrap_or(&ev.name)));
                self.run_script(&name, &ev);
            }
        }
    }

    /// Quits the programs a session's Application blocks are listening to (the timer option "Fechar os programas no fim").
    pub fn quit_sources(&mut self, idx: usize) {
        let names: Vec<String> = self.sessions[idx].blocks.iter().filter(|b| b.kind == "app_source").map(|b| b.p.text("app", "")).filter(|n| !n.is_empty() && n != "Web Browser" && n != "Any Application").collect();
        for a in audio::list_apps() {
            if a.pid > 1 && names.iter().any(|n| audio::app_matches(n, &a)) { unsafe { libc::kill(a.pid, libc::SIGTERM); } }
        }
    }

    // ---------------- per-frame work ----------------

    pub fn tick_extras(&mut self, ctx: &egui::Context) {
        // shortcuts
        if !ctx.egui_wants_keyboard_input() || ctx.input(|i| i.modifiers.command) {
            let (new, run, dup, prefs, quit) = ctx.input(|i| (
                i.modifiers.command && i.key_pressed(egui::Key::N),
                i.modifiers.command && i.key_pressed(egui::Key::R),
                i.modifiers.command && i.key_pressed(egui::Key::D),
                i.modifiers.command && i.key_pressed(egui::Key::Comma),
                i.modifiers.command && i.key_pressed(egui::Key::Q)));
            if new { self.show_templates = true; }
            if quit { self.request_quit(); }
            if run && !self.sessions.is_empty() { let c = self.cur; self.toggle_run(c); }
            if dup { self.duplicate_session(); }
            if prefs { self.x.show_prefs = true; }
            let (z, shift_z, y) = ctx.input(|i| (i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z), i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::Z), i.modifiers.command && i.key_pressed(egui::Key::Y)));
            if z && !ctx.egui_wants_keyboard_input() { self.undo(); }
            if shift_z || y { self.redo(); }
        }
        if self.shot.is_none() { self.track_undo(); }
        // selecting a block opens its settings
        if self.sel != self.x.prev_sel {
            if self.sel.is_some() { self.x.right_tab = 1; }
            self.x.prev_sel = self.sel;
        }
        // sound cards that are plugged in or out show up in the device lists
        if self.shot.is_none() && self.x.last_dev.elapsed().as_secs() >= 5 { self.x.last_dev = Instant::now(); self.devices = audio::list_devices(); }
        // sessions set to Auto Run start when the program opens
        if !self.x.launched {
            self.x.launched = true;
            for i in 0..self.sessions.len() { if self.sessions[i].auto_run == 1 && !self.is_running(self.sessions[i].uid) { self.toggle_run(i); } }
        }
        // recordings that just ended
        let mut ended: Vec<(u64, Vec<engine::FileEnd>)> = vec![];
        for r in &self.running {
            let evs: Vec<engine::FileEnd> = std::mem::take(&mut *r.engine.shared.events.lock().unwrap());
            if !evs.is_empty() { ended.push((r.uid, evs)); }
        }
        for (uid, evs) in ended { self.x.on_files_ended(uid, evs); }
        self.fire_events();
        if script::take_focus_request() { ctx.send_viewport_cmd(egui::ViewportCommand::Focus); }
        // keep the player tidy
        if self.x.player.as_ref().map(|(_, p)| p.finished()).unwrap_or(false) { self.x.player = None; }
        // save
        if self.shot.is_none() && self.x.last_save.elapsed().as_secs() >= 1 {
            self.x.last_save = Instant::now();
            if let Ok(t) = serde_json::to_string(&self.x.store) {
                if t != self.x.saved {
                    if let Some(d) = store_path().parent() { let _ = std::fs::create_dir_all(d); }
                    let _ = std::fs::write(store_path(), &t);
                    self.x.saved = t;
                }
            }
        }
    }

    // ---------------- menu bar and bars ----------------

    pub fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menubar").exact_size(26.0).frame(egui::Frame::new().fill(BACK2).inner_margin(egui::Margin::symmetric(8, 2))).show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                let has = !self.sessions.is_empty();
                ui.menu_button("Arquivo", |ui| {
                    if ui.add(egui::Button::new("Nova sessão…").shortcut_text("Ctrl+N")).clicked() { self.show_templates = true; ui.close(); }
                    if ui.add_enabled(has, egui::Button::new("Duplicar sessão").shortcut_text("Ctrl+D")).clicked() { self.duplicate_session(); ui.close(); }
                    if ui.add_enabled(has, egui::Button::new("Apagar sessão")).clicked() { self.delete_session(); ui.close(); }
                    ui.separator();
                    if ui.button("Importar sessão…").clicked() { self.x.show_io = true; self.x.io_msg.clear(); ui.close(); }
                    if ui.add_enabled(has, egui::Button::new("Exportar sessão…")).clicked() {
                        let name = self.sessions[self.cur].name.clone();
                        self.x.io_path = export_path_for(&name);
                        self.x.show_io = true; self.x.io_msg.clear(); ui.close();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Sair").shortcut_text("Ctrl+Q")).clicked() { self.request_quit(); ui.close(); }
                });
                ui.menu_button("Editar", |ui| {
                    if ui.add_enabled(!self.x.undo.is_empty() || self.x.base.as_ref().map(|b| self.sessions.get(self.cur).map(|s| s.uid == b.0 && serde_json::to_string(s).map(|t| t != b.1).unwrap_or(false)).unwrap_or(false)).unwrap_or(false), egui::Button::new("Desfazer").shortcut_text("Ctrl+Z")).clicked() { self.undo(); ui.close(); }
                    if ui.add_enabled(!self.x.redo.is_empty(), egui::Button::new("Refazer").shortcut_text("Ctrl+Shift+Z")).clicked() { self.redo(); ui.close(); }
                });
                ui.menu_button("Sessão", |ui| {
                    let running = self.sessions.get(self.cur).map(|s| self.is_running(s.uid)).unwrap_or(false);
                    if ui.add_enabled(has, egui::Button::new(if running { "Parar" } else { "Iniciar" }).shortcut_text("Ctrl+R")).clicked() { let c = self.cur; self.toggle_run(c); ui.close(); }
                    if has {
                        let mut manual = self.sessions[self.cur].manual;
                        if ui.checkbox(&mut manual, "Ligar os blocos à mão").changed() { self.sessions[self.cur].manual = manual; }
                        if ui.button("Novo agendamento").clicked() { self.sessions[self.cur].timers.push(Timer::weekdays()); self.x.right_tab = 3; ui.close(); }
                        if ui.button("Nova automação").clicked() { self.sessions[self.cur].automations.push(Automation::new()); self.x.right_tab = 4; ui.close(); }
                    }
                });
                ui.menu_button("Janela", |ui| {
                    if ui.button("Sessões").clicked() { self.x.show_home = true; ui.close(); }
                    if ui.button("Gravações").clicked() { self.x.show_recordings = true; self.x.rec_scan = None; ui.close(); }
                    if ui.button("Predefinições").clicked() { self.x.show_presets = true; ui.close(); }
                    if ui.button("Scripts").clicked() { self.x.show_scripts = true; ui.close(); }
                    if ui.button("Mini painel").clicked() { self.x.show_mini = true; ui.close(); }
                    ui.separator();
                    if ui.add(egui::Button::new("Preferências…").shortcut_text("Ctrl+,")).clicked() { self.x.show_prefs = true; ui.close(); }
                });
                ui.menu_button("Ajuda", |ui| {
                    if ui.button("Ajuda de scripts").clicked() { self.x.show_script_help = true; ui.close(); }
                    if ui.button("Sobre o Kalango Audio").clicked() { self.x.show_about = true; ui.close(); }
                });
            });
        });
    }

    /// Opens the Preferences window when an action needs the audio editor and none is set.
    fn ask_for_editor(&mut self) {
        self.x.show_prefs = true;
        self.x.prefs_note = "Escolha o editor de áudio que vai abrir as gravações.".into();
    }

    /// A slim bar that appears only when helper programs the app relies on are missing; it can be hidden.
    pub fn notice_bar(&mut self, ui: &mut egui::Ui) {
        let mut missing: Vec<&str> = vec![];
        if !engine::ffmpeg_available() { missing.push("ffmpeg (MP3/AAC/FLAC, etiquetas, transmissão, player)"); }
        if !tool_on_path("pw-record") { missing.push("pw-record (som de um programa só)"); }
        let hide = self.shot.is_some() && std::env::var("KALANGO_AUDIO_SHOW").is_err();
        if hide || missing.is_empty() || self.x.store.prefs.bar_dismissed { return; }
        egui::Panel::top("noticebar").exact_size(24.0).frame(egui::Frame::new().fill(BACK3).inner_margin(egui::Margin::symmetric(10, 2))).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new(format!("Faltam programas: {}. Essas funções ficam desligadas até você instalá-los.", missing.join(", "))).small().color(WARN));
                if ui.small_button("Ver…").clicked() { self.x.show_prefs = true; }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Esconder este aviso").clicked() { self.x.store.prefs.bar_dismissed = true; }
                });
            });
        });
    }

    /// How many Recorder blocks are writing (or holding open) a file right now, across the running sessions.
    pub fn recordings_running(&self) -> usize {
        self.running.iter().map(|r| {
            let Some(s) = self.sessions.iter().find(|s| s.uid == r.uid) else { return 0 };
            let st = r.engine.shared.rec.lock().unwrap().clone();
            s.blocks.iter().filter(|b| b.kind == "recorder" && b.enabled)
                .filter(|b| st.get(&b.id).map(|x| x.state != "finished" && !x.state.starts_with("error")).unwrap_or(true)).count()
        }).sum()
    }

    /// Quitting while something is being recorded asks first, so files are closed properly; otherwise the app just closes.
    pub fn request_quit(&mut self) {
        if self.recordings_running() == 0 { self.x.quit_now = true; } else { self.x.quit_state = 1; }
    }

    fn quit_dialog(&mut self, ctx: &egui::Context) {
        if self.x.quit_state == 0 { return; }
        if self.x.quit_state == 2 {
            // one frame later: stop everything (this waits for files to be finalized), then close
            for i in 0..self.sessions.len() { if self.is_running(self.sessions[i].uid) { self.toggle_run(i); } }
            self.x.quit_state = 0;
            self.x.quit_now = true;
            return;
        }
        let mut cancel = false;
        let mut go = false;
        let n = self.recordings_running();
        egui::Window::new("Sair do Kalango Audio?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            ui.label(if n == 1 { "Há uma gravação em andamento.".to_string() } else { format!("Há {n} gravações em andamento.") });
            ui.label(egui::RichText::new("Ao sair, as gravações são encerradas e os arquivos fechados antes de a janela fechar.").small().color(FORE2));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Parar e sair").clicked() { go = true; }
                if ui.button("Cancelar").clicked() { cancel = true; }
            });
        });
        if go { self.x.quit_state = 2; }
        if cancel { self.x.quit_state = 0; }
    }

    // ---------------- right side tabs ----------------

    pub fn right_panel_tabs(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("right").exact_size(330.0).frame(egui::Frame::new().fill(SIDE).inner_margin(egui::Margin::same(10))).show(ui, |ui| {
            if self.sessions.is_empty() { return; }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, t) in ["Blocos", "Ajustes", "Gravações", "Agenda", "Automações", "Sessão"].iter().enumerate() {
                    if ui.selectable_label(self.x.right_tab == i, *t).clicked() { self.x.right_tab = i; }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| match self.x.right_tab {
                0 => self.tab_blocks(ui),
                1 => self.tab_settings(ui),
                2 => self.tab_recordings(ui),
                3 => self.tab_timers(ui),
                4 => self.tab_automations(ui),
                _ => self.tab_info(ui),
            });
        });
    }

    fn tab_blocks(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.x.blocks_filter).hint_text("Procurar blocos").desired_width(f32::INFINITY));
        ui.label(egui::RichText::new(if self.sessions[self.cur].manual { "Clique num bloco para adicioná-lo. Os fios são ligados à mão." } else { "Clique num bloco para pô-lo depois do selecionado; ele se liga sozinho." }).small().color(FORE2));
        let filter = self.x.blocks_filter.trim().to_lowercase();
        let mut add: Option<&'static str> = None;
        let groups = [(Cat::Source, "Fontes"), (Cat::Output, "Saídas"), (Cat::Effect, "Efeitos"), (Cat::Meter, "Medidores"), (Cat::Advanced, "Avançado")];
        for (cat, title) in groups {
            let items: Vec<&KindInfo> = KINDS.iter().filter(|k| k.cat == cat && (filter.is_empty() || k.title.to_lowercase().contains(&filter) || k.desc.to_lowercase().contains(&filter))).collect();
            if items.is_empty() { continue; }
            egui::CollapsingHeader::new(egui::RichText::new(title).color(FORE1)).default_open(cat == Cat::Source || cat == Cat::Output || !filter.is_empty()).open(if filter.is_empty() { None } else { Some(true) }).show(ui, |ui| {
                for k in items {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
                    if resp.hovered() { ui.painter().rect_filled(rect, 6.0, BACK3); }
                    ui.painter().rect_filled(Rect::from_min_size(rect.left_top() + Vec2::new(4.0, 6.0), Vec2::new(28.0, 28.0)), 6.0, kind_color(k.cat));
                    let initial: String = k.title.chars().take(1).collect();
                    ui.painter().text(Pos2::new(rect.left() + 18.0, rect.center().y), Align2::CENTER_CENTER, initial, FontId::proportional(14.0), ON_THEME);
                    ui.painter().text(rect.left_top() + Vec2::new(42.0, 6.0), Align2::LEFT_TOP, k.title, FontId::proportional(13.5), FORE1);
                    let d: String = if k.desc.chars().count() > 32 { format!("{}…", k.desc.chars().take(31).collect::<String>()) } else { k.desc.to_string() };
                    ui.painter().text(rect.left_top() + Vec2::new(42.0, 23.0), Align2::LEFT_TOP, d, FontId::proportional(10.5), FORE2);
                    if resp.on_hover_text(k.desc).clicked() { add = Some(k.id); }
                }
            });
        }
        if let Some(k) = add { self.add_from_library(k); }
    }

    fn tab_settings(&mut self, ui: &mut egui::Ui) {
        let Some(sel) = self.sel else {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("Selecione um bloco para ver os ajustes dele.").color(FORE2));
            ui.label(egui::RichText::new("Clique num bloco no quadro. Com o botão direito há mais opções.").small().color(FORE2));
            return;
        };
        let uid = self.sessions[self.cur].uid;
        let rec = self.running.iter().find(|r| r.uid == uid).map(|r| r.engine.shared.rec.lock().unwrap().clone());
        let errs = self.running.iter().find(|r| r.uid == uid).map(|r| r.engine.shared.errors.lock().unwrap().clone());
        let devices = self.devices.clone();
        let apps_now = self.apps.lock().unwrap().clone();
        let is_app_block = self.sessions[self.cur].block(sel).map(|b| b.kind == "app_source").unwrap_or(false);
        self.want_apps.store(is_app_block, std::sync::atomic::Ordering::Relaxed);
        let blocks_snapshot = self.sessions[self.cur].blocks.clone();
        let mut delete = false;
        let mut dup = false;
        let user_presets = self.x.store.presets.clone();
        let mut save_preset: Option<Preset> = None;
        {
            let s = &mut self.sessions[self.cur];
            let Some(b) = s.blocks.iter_mut().find(|b| b.id == sel) else { return };
            let info = kind_info(&b.kind);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(info.map(|i| i.title).unwrap_or("Bloco")).strong().size(16.0).color(FORE1));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { ui.checkbox(&mut b.enabled, "Ligado"); });
            });
            if let Some(i) = info {
                ui.label(egui::RichText::new(i.desc).small().color(FORE2));
                if !i.working { ui.label(egui::RichText::new("Ainda não disponível: o áudio passa sem mudança.").small().color(WARN)); }
            }
            // presets
            let kind = b.kind.clone();
            let mut choices: Vec<(String, Preset, bool)> = builtin_presets().into_iter().filter(|p| p.kind == kind).map(|p| (p.name.clone(), p, true)).collect();
            choices.extend(user_presets.iter().filter(|p| p.kind == kind).map(|p| (p.name.clone(), p.clone(), false)));
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Predefinição").color(FORE2));
                egui::ComboBox::from_id_salt("presetpick").width(120.0).selected_text("Escolher…").show_ui(ui, |ui| {
                    if choices.is_empty() { ui.label(egui::RichText::new("(nenhuma ainda)").small().color(FORE2)); }
                    for (n, p, builtin) in &choices {
                        if ui.selectable_label(false, if *builtin { n.clone() } else { format!("★ {n}") }).clicked() { apply_preset(b, p); }
                    }
                });
                if ui.small_button("Salvar como…").clicked() { self.x.preset_name = format!("Meu ajuste · {}", info.map(|i| i.title).unwrap_or("bloco")); }
            });
            if !self.x.preset_name.is_empty() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.x.preset_name).desired_width(170.0));
                    if ui.small_button("Salvar").clicked() {
                        let mut p = b.p.clone();
                        for k in ["pause", "split_req", "learn_seq", "jump_seq", "live_seq", "jump_s", "jump_live"] { p.n.remove(k); }
                        save_preset = Some(Preset { kind: kind.clone(), name: self.x.preset_name.clone(), p });
                    }
                    if ui.small_button("Cancelar").clicked() { self.x.preset_name.clear(); }
                });
            }
            ui.add_space(6.0);
            let ups: Vec<(u64, String)> = { let mut v: Vec<(u64, String)> = vec![]; for x in blocks_snapshot.iter().filter(|x| x.outs.contains(&sel)) { v.push((x.id, kind_info(&x.kind).map(|i| i.title.to_string()).unwrap_or_default())); } v.sort_by_key(|x| x.0); v };
            let st = rec.as_ref().and_then(|m| m.get(&sel).cloned());
            let err = errs.as_ref().and_then(|m| m.get(&sel).cloned());
            block_settings(ui, b, &devices, &apps_now, &ups, st, err);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Duplicar bloco").clicked() { dup = true; }
                if ui.button("Apagar bloco").clicked() { delete = true; }
            });
        }
        if let Some(p) = save_preset {
            self.x.store.presets.retain(|q| !(q.kind == p.kind && q.name == p.name));
            self.x.store.presets.push(p);
            self.x.preset_name.clear();
        }
        if dup {
            let cur = self.cur;
            if let Some(src) = self.sessions[cur].block(sel).cloned() {
                let nid = self.sessions[cur].add_block(&src.kind, src.x + 1.0, src.y + 4.0);
                if let Some(nb) = self.sessions[cur].blocks.iter_mut().find(|x| x.id == nid) { nb.p = src.p.clone(); }
                self.sel = Some(nid);
            }
        }
        if delete { self.remove_block_smart(sel); }
    }

    /// Recordings made by this session (the files in its Recorder folders), with the Details and Actions buttons.
    fn tab_recordings(&mut self, ui: &mut egui::Ui) {
        if self.x.rec_scan.map(|t| t.elapsed().as_secs() > 5).unwrap_or(true) { self.scan_recordings(); }
        let folders = self.session_folders(self.cur);
        let mine: Vec<usize> = self.x.recs.iter().enumerate().filter(|(_, r)| folders.iter().any(|f| f.to_string_lossy() == r.folder)).map(|(i, _)| i).collect();
        let uid = self.sessions[self.cur].uid;
        // a file that is being written right now
        let live: Vec<String> = self.running.iter().find(|r| r.uid == uid).map(|r| r.engine.shared.rec.lock().unwrap().values().filter(|v| v.state == "recording").map(|v| v.file.clone()).collect()).unwrap_or_default();
        if mine.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nenhuma gravação ainda").strong().color(FORE2));
                ui.label(egui::RichText::new("As gravações feitas nesta sessão aparecem aqui.").small().color(FORE2));
            });
            return;
        }
        let mut inspect: Option<usize> = None;
        let mut action: Option<(usize, &'static str)> = None;
        for i in mine {
            let r = &self.x.recs[i];
            let is_live = live.iter().any(|f| *f == r.path.to_string_lossy());
            ui.horizontal(|ui| {
                let sel = self.x.rec_sel == Some(i);
                if ui.selectable_label(sel, if is_live { format!("● {}", r.name) } else { r.name.clone() }).clicked() { self.x.rec_sel = Some(i); }
            });
            ui.label(egui::RichText::new(format!("{}  ·  {}  ·  {}", fmt_epoch(r.modified), r.dur.map(fmt_time).unwrap_or_else(|| "—".into()), fmt_size(r.size))).small().color(FORE2));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let has = self.x.rec_sel.is_some();
            if ui.add_enabled(has, egui::Button::new("Detalhes")).on_hover_text("Abre a gravação selecionada na janela Gravações").clicked() { inspect = self.x.rec_sel; }
            ui.menu_button("Ações ▾", |ui| {
                for (label, key) in [("Mostrar em Arquivos", "reveal"), ("Tocar", "play"), ("Abrir no editor de áudio", "edit"), ("Abrir no player de música", "music"), ("Copiar caminho", "share"), ("Mover para a lixeira", "trash")] {
                    if ui.add_enabled(has, egui::Button::new(label)).clicked() { if let Some(i) = self.x.rec_sel { action = Some((i, key)); } ui.close(); }
                }
            });
        });
        if let Some(i) = inspect { self.x.show_recordings = true; self.select_recording(i); }
        if let Some((i, key)) = action { self.file_action(i, key); }
    }

    fn tab_timers(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.sessions[self.cur];
        ui.label(egui::RichText::new("Inicia e para esta sessão sozinha, no horário marcado.").small().color(FORE2));
        if s.timers.is_empty() {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nenhum agendamento ainda").strong().color(FORE2));
                ui.label(egui::RichText::new("Clique em “Novo agendamento” abaixo para começar.").small().color(FORE2));
            });
        }
        let mut drop_idx: Option<usize> = None;
        for (i, t) in s.timers.iter_mut().enumerate() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut t.on, "Ligado");
                    egui::ComboBox::from_id_salt(format!("tmode{i}")).selected_text(if t.mode == 0 { "Num dia" } else { "Toda semana" }).show_ui(ui, |ui| {
                        if ui.selectable_label(t.mode == 0, "Num dia").clicked() { t.mode = 0; if t.date == 0 { t.date = engine::local_date(); } }
                        if ui.selectable_label(t.mode == 1, "Toda semana").clicked() { t.mode = 1; }
                    });
                    if ui.small_button("✕").clicked() { drop_idx = Some(i); }
                });
                if t.mode == 0 {
                    if t.date == 0 { t.date = engine::local_date(); }
                    ui.horizontal(|ui| {
                        ui.label("Data:");
                        let (mut y, mut m, mut d) = (t.date / 10000, (t.date / 100) % 100, t.date % 100);
                        ui.add(egui::DragValue::new(&mut d).range(1..=31).custom_formatter(|v, _| format!("{:02}", v as i32)));
                        ui.label("/");
                        ui.add(egui::DragValue::new(&mut m).range(1..=12).custom_formatter(|v, _| format!("{:02}", v as i32)));
                        ui.label("/");
                        ui.add(egui::DragValue::new(&mut y).range(2020..=2100));
                        t.date = y * 10000 + m * 100 + d;
                    });
                } else {
                    ui.horizontal(|ui| {
                        // the week starts on Monday (ISO 8601), so the work days sit together
                        for (d, name) in [(1u8, "Seg"), (2, "Ter"), (3, "Qua"), (4, "Qui"), (5, "Sex"), (6, "Sáb"), (0, "Dom")] {
                            let mut on = t.days & (1 << d) != 0;
                            if ui.toggle_value(&mut on, name).changed() { if on { t.days |= 1 << d; } else { t.days &= !(1 << d); } }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Das"); 
                    let (mut h, mut m) = (t.start / 60, t.start % 60);
                    ui.add(egui::DragValue::new(&mut h).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as i32)));
                    ui.label(":");
                    ui.add(egui::DragValue::new(&mut m).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as i32)));
                    t.start = h * 60 + m;
                    ui.label("às");
                    let end = (t.start + t.minutes) % 1440;
                    let (mut eh, mut em) = (end / 60, end % 60);
                    ui.add(egui::DragValue::new(&mut eh).range(0..=23).custom_formatter(|v, _| format!("{:02}", v as i32)));
                    ui.label(":");
                    ui.add(egui::DragValue::new(&mut em).range(0..=59).custom_formatter(|v, _| format!("{:02}", v as i32)));
                    let new_end = eh * 60 + em;
                    t.minutes = if new_end > t.start { new_end - t.start } else { new_end + 1440 - t.start }.max(1);
                });
                ui.checkbox(&mut t.quit_sources, "Fechar os programas no fim").on_hover_text("Fecha os programas que os blocos Programa estão ouvindo (só os escolhidos pelo nome)");
            });
        }
        if let Some(i) = drop_idx { s.timers.remove(i); }
        if ui.button("+ Novo agendamento").clicked() { s.timers.push(Timer::weekdays()); }
    }

    fn tab_automations(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Uma automação roda um script quando algo acontece.").small().color(FORE2));
        let mut names: Vec<String> = script::BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
        names.extend(self.x.store.scripts.iter().map(|s| s.name.clone()));
        let s = &mut self.sessions[self.cur];
        if s.automations.is_empty() {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nenhuma automação ainda").strong().color(FORE2));
                ui.label(egui::RichText::new("Clique em “Nova automação” abaixo para começar.").small().color(FORE2));
            });
        }
        let mut drop_idx: Option<usize> = None;
        let mut reveal: Option<String> = None;
        for (i, a) in s.automations.iter_mut().enumerate() {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Quando:");
                    let cur = EVENTS.iter().find(|e| e.0 == a.event).map(|e| e.1).unwrap_or(EVENTS[1].1);
                    egui::ComboBox::from_id_salt(format!("ev{i}")).selected_text(cur).show_ui(ui, |ui| {
                        for (key, label) in EVENTS { if ui.selectable_label(a.event == key, label).clicked() { a.event = key.to_string(); } }
                    });
                    ui.checkbox(&mut a.on, "Ligada");
                    if ui.small_button("✕").clicked() { drop_idx = Some(i); }
                });
                ui.horizontal(|ui| {
                    ui.label("Rodar:");
                    egui::ComboBox::from_id_salt(format!("scr{i}")).selected_text(if a.script.is_empty() { "Escolha um script".to_string() } else { a.script.clone() }).show_ui(ui, |ui| {
                        for n in &names { if ui.selectable_label(&a.script == n, n).clicked() { a.script = n.clone(); } }
                    });
                    if ui.add_enabled(!a.script.is_empty(), egui::Button::new("🔍").small()).on_hover_text("Mostra o script na janela Scripts").clicked() { reveal = Some(a.script.clone()); }
                });
            });
        }
        if let Some(i) = drop_idx { s.automations.remove(i); }
        if ui.button("+ Nova automação").clicked() { s.automations.push(Automation::new()); }
        if let Some(n) = reveal {
            self.x.show_scripts = true;
            if let Some(i) = script::BUILTIN.iter().position(|(b, _)| *b == n) { self.x.script_sel = (true, i); }
            else if let Some(i) = self.x.store.scripts.iter().position(|s| s.name == n) { self.x.script_sel = (false, i); }
        }
    }

    fn tab_info(&mut self, ui: &mut egui::Ui) {
        let running = self.is_running(self.sessions[self.cur].uid);
        let s = &mut self.sessions[self.cur];
        ui.label(egui::RichText::new("SESSÃO").small().color(FORE2));
        ui.label("Nome:");
        ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(f32::INFINITY));
        ui.label("Anotações:");
        ui.add(egui::TextEdit::multiline(&mut s.notes).desired_rows(5).desired_width(f32::INFINITY).hint_text("Anotações sobre esta sessão"));
        ui.add_space(6.0);
        let mut auto = !s.manual;
        if ui.checkbox(&mut auto, "Ligar os blocos sozinho").on_hover_text("Ligado: os blocos se conectam automaticamente. Desligado: você liga os fios à mão.").changed() { s.manual = !auto; }
        ui.horizontal(|ui| {
            ui.label("Iniciar:");
            egui::ComboBox::from_id_salt("autorun").selected_text(if s.auto_run == 1 { "Ao abrir o programa" } else { "Só à mão" }).show_ui(ui, |ui| {
                if ui.selectable_label(s.auto_run == 0, "Só à mão").clicked() { s.auto_run = 0; }
                if ui.selectable_label(s.auto_run == 1, "Ao abrir o programa").clicked() { s.auto_run = 1; }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Taxa de amostragem:");
            ui.add_enabled_ui(!running, |ui| {
                let cur = if s.sample_rate == 0 { 48000 } else { s.sample_rate };
                egui::ComboBox::from_id_salt("srate").selected_text(format!("{} Hz", cur)).show_ui(ui, |ui| {
                    for r in [22050u32, 32000, 44100, 48000, 88200, 96000] { if ui.selectable_label(cur == r, format!("{r} Hz")).clicked() { s.sample_rate = if r == 48000 { 0 } else { r }; } }
                });
            });
        });
        ui.label(egui::RichText::new("O som é processado a 48 kHz; os arquivos usam a taxa da sessão, a menos que o bloco Gravador escolha outra. Não muda com a sessão rodando.").small().color(FORE2));
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("{} blocos · {} agendamentos · {} automações", s.blocks.len(), s.timers.len(), s.automations.len())).small().color(FORE2));
    }

    // ---------------- recordings ----------------

    fn session_folders(&self, idx: usize) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.sessions[idx].blocks.iter().filter(|b| b.kind == "recorder").map(|b| expand_home(&b.p.text("folder", ""))).collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn scan_recordings(&mut self) {
        let mut folders: Vec<PathBuf> = vec![expand_home(&self.x.store.prefs.default_folder)];
        for i in 0..self.sessions.len() { folders.extend(self.session_folders(i)); }
        folders.sort();
        folders.dedup();
        let selected_path = self.x.rec_sel.and_then(|i| self.x.recs.get(i)).map(|r| r.path.clone());
        let mut v: Vec<RecFile> = vec![];
        for f in folders {
            let Ok(rd) = std::fs::read_dir(&f) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
                if !["wav", "aiff", "aif", "mp3", "m4a", "mp4", "flac"].contains(&ext.as_str()) { continue; }
                let Ok(md) = e.metadata() else { continue };
                let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                let dur = if ext == "wav" { hound::WavReader::open(&p).ok().map(|r| r.duration() as f64 / r.spec().sample_rate as f64) } else { None };
                v.push(RecFile { name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), folder: f.to_string_lossy().to_string(), path: p, size: md.len(), modified, dur });
            }
        }
        v.sort_by(|a, b| b.modified.cmp(&a.modified));
        v.truncate(300);
        self.x.recs = v;
        self.x.rec_scan = Some(Instant::now());
        self.x.rec_sel = selected_path.and_then(|p| self.x.recs.iter().position(|r| r.path == p));
    }

    fn select_recording(&mut self, i: usize) {
        self.x.rec_sel = Some(i);
        let Some(r) = self.x.recs.get(i).cloned() else { return };
        if self.x.insp.for_path.as_deref() == Some(r.path.as_path()) { return; }
        self.x.insp = Insp {
            for_path: Some(r.path.clone()), name_edit: r.name.clone(), dur: r.dur, tags_open: true,
            tags: if media::have("ffprobe") { media::read_tags(&r.path) } else { Tags::default() },
            ..Default::default()
        };
        if self.x.insp.dur.is_none() && media::have("ffprobe") {
            let d = media::duration(&r.path);
            self.x.insp.dur = d;
            if let Some(rr) = self.x.recs.get_mut(i) { rr.dur = d; }
        }
        // the player belongs to one file
        if self.x.player.as_ref().map(|(p, _)| *p != r.path).unwrap_or(false) { self.x.player = None; }
    }

    fn file_action(&mut self, i: usize, key: &str) {
        let Some(r) = self.x.recs.get(i).cloned() else { return };
        match key {
            "reveal" => { let _ = std::process::Command::new("xdg-open").arg(&r.folder).spawn(); }
            "play" | "music" => { let _ = std::process::Command::new("xdg-open").arg(&r.path).spawn(); }
            "edit" => {
                let ed = self.x.store.prefs.audio_editor.trim().to_string();
                if ed.is_empty() { self.ask_for_editor(); } else { let _ = std::process::Command::new(ed).arg(&r.path).spawn(); }
            }
            "share" => { self.x.insp.msg = format!("Caminho copiado: {}", r.path.display()); }
            "trash" => {
                let ok = media::trash(&r.path);
                self.x.insp.msg = if ok { "Movido para a lixeira.".into() } else { "Não foi possível mover para a lixeira (o gio está instalado?).".into() };
                self.scan_recordings();
            }
            _ => {}
        }
    }

    fn recordings_ui(&mut self, ui: &mut egui::Ui) {
        if self.x.rec_scan.map(|t| t.elapsed().as_secs() > 5).unwrap_or(true) { self.scan_recordings(); }
        let mut pick: Option<usize> = None;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(230.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{} gravações", self.x.recs.len())).color(FORE2));
                    if ui.small_button("Atualizar").clicked() { self.scan_recordings(); }
                });
                egui::ScrollArea::vertical().max_height(420.0).id_salt("recscroll").show(ui, |ui| {
                    for (i, r) in self.x.recs.iter().enumerate() {
                        if ui.selectable_label(self.x.rec_sel == Some(i), &r.name).on_hover_text(fmt_epoch(r.modified)).clicked() { pick = Some(i); }
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| self.inspector_details(ui));
        });
        if let Some(i) = pick { self.select_recording(i); }
    }

    fn inspector_details(&mut self, ui: &mut egui::Ui) {
        ui.set_width(380.0);
        let Some(i) = self.x.rec_sel.filter(|i| *i < self.x.recs.len()) else {
            ui.label(egui::RichText::new("Nenhuma gravação selecionada").color(FORE2));
            return;
        };
        let r = self.x.recs[i].clone();
        let uid = self.sessions.get(self.cur).map(|s| s.uid).unwrap_or(0);
        let live = self.running.iter().find(|x| x.uid == uid).map(|x| x.engine.shared.rec.lock().unwrap().values().any(|v| v.state == "recording" && v.file == r.path.to_string_lossy())).unwrap_or(false);
        ui.label(egui::RichText::new(if live { "GRAVANDO AGORA" } else { "INFORMAÇÕES" }).small().color(if live { LIVE } else { FORE2 }));
        // file name
        ui.horizontal(|ui| {
            ui.label("Nome:");
            ui.add(egui::TextEdit::singleline(&mut self.x.insp.name_edit).desired_width(220.0));
            if ui.add_enabled(!live && self.x.insp.name_edit != r.name && !self.x.insp.name_edit.trim().is_empty(), egui::Button::new("Renomear")).clicked() {
                let dest = r.path.with_file_name(self.x.insp.name_edit.trim());
                if dest.exists() { self.x.insp.msg = "Já existe um arquivo com esse nome.".into(); }
                else if std::fs::rename(&r.path, &dest).is_ok() { self.x.insp.msg = "Renomeado.".into(); self.x.insp.for_path = None; self.scan_recordings(); }
                else { self.x.insp.msg = "Não foi possível renomear o arquivo.".into(); }
            }
        });
        let ext = r.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_uppercase();
        egui::Grid::new("inspgrid").num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
            ui.label(egui::RichText::new("Formato").color(FORE2)); ui.label(ext); ui.end_row();
            ui.label(egui::RichText::new("Tamanho").color(FORE2)); ui.label(fmt_size(r.size)); ui.end_row();
            ui.label(egui::RichText::new("Duração").color(FORE2)); ui.label(self.x.insp.dur.map(fmt_time).unwrap_or_else(|| "—".into())); ui.end_row();
            ui.label(egui::RichText::new("Gravado em").color(FORE2)); ui.label(fmt_epoch(r.modified)); ui.end_row();
        });
        // player
        ui.add_space(6.0);
        ui.label(egui::RichText::new("OUVIR").small().color(FORE2));
        let dur = self.x.insp.dur.unwrap_or(0.0);
        let playing = self.x.player.as_ref().map(|(p, pl)| *p == r.path && !pl.paused.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false);
        let pos = self.x.player.as_ref().filter(|(p, _)| *p == r.path).map(|(_, pl)| pl.position()).unwrap_or(0.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!live && tool_on_path("ffmpeg"), egui::Button::new(if playing { "⏸" } else { "▶" })).clicked() {
                match &self.x.player {
                    Some((p, pl)) if *p == r.path => { let v = pl.paused.load(std::sync::atomic::Ordering::Relaxed); pl.paused.store(!v, std::sync::atomic::Ordering::Relaxed); }
                    _ => { self.x.player = Some((r.path.clone(), Player::start(&r.path, 0.0))); }
                }
            }
            ui.label(fmt_clock(pos));
            let mut p = pos as f32;
            if dur > 0.0 && ui.add(egui::Slider::new(&mut p, 0.0..=dur as f32).show_value(false)).drag_stopped() {
                let paused = self.x.player.as_ref().map(|(_, pl)| pl.paused.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(false);
                let np = Player::start(&r.path, p as f64);
                if paused { np.paused.store(true, std::sync::atomic::Ordering::Relaxed); }
                self.x.player = Some((r.path.clone(), np));
            }
            ui.label(format!("-{}", fmt_clock((dur - pos).max(0.0))));
        });
        // tags
        ui.add_space(6.0);
        let can_tag = !live && tool_on_path("ffmpeg");
        egui::CollapsingHeader::new("Etiquetas").default_open(self.x.insp.tags_open).show(ui, |ui| {
            let t = &mut self.x.insp.tags;
            egui::Grid::new("tagsgrid").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                let mut row = |ui: &mut egui::Ui, label: &str, v: &mut String| { ui.label(label); ui.add(egui::TextEdit::singleline(v).desired_width(240.0)); ui.end_row(); };
                row(ui, "Título", &mut t.title);
                row(ui, "Artista", &mut t.artist);
                row(ui, "Álbum", &mut t.album);
                row(ui, "Gênero", &mut t.genre);
                row(ui, "Ano", &mut t.year);
                ui.label("Faixa");
                ui.horizontal(|ui| { ui.add(egui::TextEdit::singleline(&mut t.track).desired_width(50.0)); ui.label("de"); ui.add(egui::TextEdit::singleline(&mut t.track_count).desired_width(50.0)); });
                ui.end_row();
                ui.label("Disco");
                ui.horizontal(|ui| { ui.add(egui::TextEdit::singleline(&mut t.disc).desired_width(50.0)); ui.label("de"); ui.add(egui::TextEdit::singleline(&mut t.disc_count).desired_width(50.0)); });
                ui.end_row();
                row(ui, "Comentário", &mut t.comment);
                row(ui, "Capa (arquivo de imagem)", &mut t.art);
            });
            ui.checkbox(&mut t.compilation, "Coletânea");
            ui.horizontal(|ui| {
                if ui.add_enabled(can_tag, egui::Button::new("Salvar etiquetas")).on_hover_text("Grava as etiquetas dentro do arquivo").clicked() {
                    let tags = self.x.insp.tags.clone();
                    self.x.insp.msg = match media::write_tags(&r.path, &tags) { Ok(()) => "Etiquetas salvas.".into(), Err(e) => format!("Não foi possível salvar as etiquetas: {e}") };
                }
                if !can_tag { ui.label(egui::RichText::new(if live { "Espere a gravação terminar." } else { "É preciso ter o ffmpeg para gravar etiquetas." }).small().color(FORE2)); }
            });
        });
        ui.add_space(6.0);
        let mut action: Option<&'static str> = None;
        ui.horizontal(|ui| {
            if ui.button("Mostrar em Arquivos").clicked() { action = Some("reveal"); }
            if ui.button("Editor de áudio").on_hover_text("Abre no editor de áudio escolhido nas Preferências").clicked() { action = Some("edit"); }
            if ui.button("Player de música").on_hover_text("Abre no player de música do sistema").clicked() { action = Some("music"); }
            if ui.button("Copiar caminho").clicked() { ui.ctx().copy_text(r.path.to_string_lossy().to_string()); action = Some("share"); }
            let label = if self.x.insp.trash_confirm { "Mover mesmo?" } else { "Lixeira" };
            if ui.add_enabled(!live, egui::Button::new(label)).clicked() {
                if self.x.insp.trash_confirm { self.x.insp.trash_confirm = false; action = Some("trash"); } else { self.x.insp.trash_confirm = true; }
            }
        });
        if let Some(k) = action { self.file_action(i, k); }
        if !self.x.insp.msg.is_empty() { ui.label(egui::RichText::new(&self.x.insp.msg).small().color(FORE1)); }
    }

    // ---------------- floating windows ----------------

    pub fn extra_windows(&mut self, ctx: &egui::Context) {
        self.template_chooser(ctx);
        self.quit_dialog(ctx);
        // messages from scripts
        let mut close_dialog = false;
        if let Some(m) = self.x.dialogs.first().cloned() {
            egui::Window::new("Mensagem do script").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.label(&m);
                if ui.button("OK").clicked() { close_dialog = true; }
            });
        }
        if close_dialog { self.x.dialogs.remove(0); }
        let mut open = self.x.show_home;
        if open { egui::Window::new("Sessões").open(&mut open).default_size([620.0, 360.0]).show(ctx, |ui| self.home_ui(ui)); }
        self.x.show_home = open;
        let mut open = self.x.show_recordings;
        if open { egui::Window::new("Gravações").open(&mut open).default_size([700.0, 520.0]).show(ctx, |ui| self.recordings_ui(ui)); }
        self.x.show_recordings = open;
        let mut open = self.x.show_presets;
        if open { egui::Window::new("Predefinições").open(&mut open).default_size([420.0, 380.0]).show(ctx, |ui| self.presets_ui(ui)); }
        self.x.show_presets = open;
        let mut open = self.x.show_scripts;
        if open { egui::Window::new("Scripts").open(&mut open).default_size([700.0, 480.0]).show(ctx, |ui| self.scripts_ui(ui)); }
        self.x.show_scripts = open;
        let mut open = self.x.show_script_help;
        if open { egui::Window::new("Ajuda de scripts").open(&mut open).default_size([560.0, 420.0]).show(ctx, |ui| { egui::ScrollArea::vertical().show(ui, |ui| { ui.label(egui::RichText::new(script::HELP).monospace().small()); }); }); }
        self.x.show_script_help = open;
        let mut open = self.x.show_mini;
        if open { egui::Window::new("Mini painel").open(&mut open).default_size([260.0, 280.0]).anchor(Align2::RIGHT_TOP, Vec2::new(-340.0, 80.0)).show(ctx, |ui| self.mini_ui(ui)); }
        self.x.show_mini = open;
        let mut open = self.x.show_prefs;
        if open { egui::Window::new("Preferências").open(&mut open).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| self.prefs_ui(ui)); }
        if !open { self.x.prefs_note.clear(); }
        self.x.show_prefs = open;
        let mut open = self.x.show_about;
        if open { egui::Window::new("Sobre").open(&mut open).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| about_ui(ui)); }
        self.x.show_about = open;
        let mut open = self.x.show_io;
        if open { egui::Window::new("Importar ou exportar sessão").open(&mut open).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| self.io_ui(ui)); }
        self.x.show_io = open;
    }

    /// The Sessions window: every session in a sortable table; columns can be hidden.
    fn home_ui(&mut self, ui: &mut egui::Ui) {
        if self.x.rec_scan.is_none() { self.scan_recordings(); }
        ui.horizontal(|ui| {
            if ui.button("+ Nova sessão").clicked() { self.show_templates = true; }
            if ui.add_enabled(self.x.home_sel.is_some(), egui::Button::new("− Apagar sessão")).clicked() {
                if let Some(i) = self.x.home_sel.take() { if i < self.sessions.len() { self.cur = i; self.delete_session(); } }
            }
            ui.menu_button("Colunas ▾", |ui| {
                for (k, name) in HOME_COLUMNS.iter().enumerate() {
                    if k == 0 { ui.add_enabled(false, egui::Checkbox::new(&mut true.clone(), *name)); } else { ui.checkbox(&mut self.x.home_cols[k], *name); }
                }
            });
        });
        ui.add_space(4.0);
        let mut rows: Vec<(usize, String, String, String, usize, usize)> = (0..self.sessions.len()).map(|i| {
            let s = &self.sessions[i];
            let folders = self.session_folders(i);
            let recs = self.x.recs.iter().filter(|r| folders.iter().any(|f| f.to_string_lossy() == r.folder)).count();
            let status = if self.is_running(s.uid) { "Rodando" } else { "Parada" };
            (i, s.name.clone(), status.to_string(), if s.auto_run == 1 { "Ao abrir".to_string() } else { "À mão".to_string() }, recs, s.timers.len())
        }).collect();
        let (col, asc) = self.x.home_sort;
        rows.sort_by(|a, b| {
            let o = match col { 0 => a.1.to_lowercase().cmp(&b.1.to_lowercase()), 1 => a.2.cmp(&b.2), 2 => a.3.cmp(&b.3), 3 => a.4.cmp(&b.4), _ => a.5.cmp(&b.5) };
            if asc { o } else { o.reverse() }
        });
        let mut toggle: Option<usize> = None;
        let mut open_idx: Option<usize> = None;
        egui::Grid::new("homegrid").striped(true).num_columns(6).spacing([18.0, 6.0]).show(ui, |ui| {
            ui.label("");
            for (k, name) in HOME_COLUMNS.iter().enumerate() {
                if !self.x.home_cols[k] { continue; }
                let arrow = if self.x.home_sort.0 == k { if self.x.home_sort.1 { " ▲" } else { " ▼" } } else { "" };
                if ui.selectable_label(false, egui::RichText::new(format!("{name}{arrow}")).small().color(FORE2)).clicked() {
                    self.x.home_sort = if self.x.home_sort.0 == k { (k, !self.x.home_sort.1) } else { (k, true) };
                }
            }
            ui.end_row();
            for (i, name, status, auto, recs, timers) in &rows {
                let running = status == "Rodando";
                if ui.add(egui::Button::new(egui::RichText::new(if running { "■" } else { "▶" }).color(if running { ON_THEME } else { FORE1 })).small().fill(if running { LIVE } else { BACK3 })).on_hover_text(if running { "Parar esta sessão" } else { "Iniciar esta sessão" }).clicked() { toggle = Some(*i); }
                if self.x.home_cols[0] {
                    let r = ui.selectable_label(self.x.home_sel == Some(*i), name);
                    if r.clicked() { self.x.home_sel = Some(*i); }
                    if r.double_clicked() { open_idx = Some(*i); }
                }
                if self.x.home_cols[1] { ui.label(egui::RichText::new(status).color(if running { LIVE } else { FORE2 })); }
                if self.x.home_cols[2] { ui.label(auto); }
                if self.x.home_cols[3] { ui.label(recs.to_string()); }
                if self.x.home_cols[4] { ui.label(timers.to_string()); }
                ui.end_row();
            }
        });
        if rows.is_empty() { ui.label(egui::RichText::new("Nenhuma sessão ainda. Clique em “Nova sessão” para criar uma.").color(FORE2)); }
        if let Some(i) = toggle { self.toggle_run(i); }
        if let Some(i) = open_idx { self.cur = i; self.sel = None; }
    }

    fn template_chooser(&mut self, ctx: &egui::Context) {
        if !self.show_templates { return; }
        let mut open = true;
        let mut chosen: Option<usize> = None;
        egui::Window::new("Nova sessão").open(&mut open).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| { egui::ScrollArea::vertical().max_height(380.0).id_salt("tplscroll").show(ui, |ui| {
                    ui.set_width(230.0);
                    for (n, t) in self.templates.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(226.0, 44.0), Sense::click());
                        let sel = self.x.tpl_sel == n;
                        ui.painter().rect_filled(rect, 6.0, if sel { THEME_DARK } else if resp.hovered() { RAISED } else { BACK3 });
                        ui.painter().text(rect.left_top() + Vec2::new(10.0, 6.0), Align2::LEFT_TOP, &t.name, FontId::proportional(13.5), FORE1);
                        let sub: String = t.subtitle.chars().take(34).collect();
                        ui.painter().text(rect.left_top() + Vec2::new(10.0, 25.0), Align2::LEFT_TOP, sub, FontId::proportional(11.0), FORE2);
                        if resp.clicked() { self.x.tpl_sel = n; }
                        if resp.double_clicked() { chosen = Some(n); }
                    }
                }); });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(280.0);
                    if let Some(t) = self.templates.get(self.x.tpl_sel) {
                        ui.label(egui::RichText::new(&t.name).strong().size(16.0).color(FORE1));
                        ui.label(egui::RichText::new(&t.subtitle).color(FORE2));
                        ui.add_space(6.0);
                        if !t.desc.trim().is_empty() { ui.label(&t.desc); }
                        ui.add_space(8.0);
                        // little picture of the chain
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(270.0, 70.0), Sense::hover());
                        ui.painter().rect_filled(rect, 6.0, BACK2);
                        if !t.blocks.is_empty() {
                            let minx = t.blocks.iter().map(|b| b.x).fold(f32::MAX, f32::min);
                            let maxx = t.blocks.iter().map(|b| b.x).fold(f32::MIN, f32::max).max(minx + 1.0);
                            let miny = t.blocks.iter().map(|b| b.y).fold(f32::MAX, f32::min);
                            let maxy = t.blocks.iter().map(|b| b.y).fold(f32::MIN, f32::max).max(miny + 1.0);
                            let pos = |b: &TplBlock| Pos2::new(rect.left() + 20.0 + (b.x - minx) / (maxx - minx) * (rect.width() - 40.0), rect.top() + 14.0 + (b.y - miny) / (maxy - miny) * (rect.height() - 28.0));
                            for b in &t.blocks { for o in &b.out { if let Some(c) = t.blocks.get(*o) { ui.painter().line_segment([pos(b), pos(c)], Stroke::new(1.5, THEME)); } } }
                            for b in &t.blocks {
                                let cat = kind_info(&b.kind).map(|i| i.cat).unwrap_or(Cat::Effect);
                                ui.painter().circle_filled(pos(b), 6.0, kind_color(cat));
                            }
                        }
                        ui.label(egui::RichText::new(format!("{} blocos", t.blocks.len())).small().color(FORE2));
                    }
                    ui.add_space(10.0);
                    if ui.add(egui::Button::new(egui::RichText::new("Criar sessão").color(ON_THEME)).fill(THEME).min_size(Vec2::new(110.0, 28.0))).clicked() { chosen = Some(self.x.tpl_sel); }
                });
            });
        });
        if let Some(n) = chosen {
            if let Some(t) = self.templates.get(n) {
                let s = t.instantiate();
                self.sessions.push(s);
                self.cur = self.sessions.len() - 1;
                self.sel = None;
                self.show_templates = false;
            }
        }
        if !open { self.show_templates = false; }
    }

    fn mini_ui(&mut self, ui: &mut egui::Ui) {
        let mut toggle: Option<usize> = None;
        let mut pick: Option<usize> = None;
        for (i, s) in self.sessions.iter().enumerate() {
            let running = self.is_running(s.uid);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, if running { LIVE } else { DIM });
                if ui.selectable_label(i == self.cur, &s.name).clicked() { pick = Some(i); }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(egui::RichText::new(if running { "■ Parar" } else { "▶ Iniciar" }).color(ON_THEME)).fill(if running { LIVE } else { GREEN }).small()).clicked() { toggle = Some(i); }
                });
            });
        }
        if self.sessions.is_empty() { ui.label(egui::RichText::new("Nenhuma sessão ainda. Crie uma em “Nova sessão”.").small().color(FORE2)); }
        if let Some(i) = pick { self.cur = i; self.sel = None; }
        if let Some(i) = toggle { self.toggle_run(i); }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("+ Nova sessão").clicked() { self.show_templates = true; }
            if ui.button("Todas as sessões").clicked() { self.x.show_home = true; }
        });
    }

    fn prefs_ui(&mut self, ui: &mut egui::Ui) {
        ui.set_width(430.0);
        if !self.x.prefs_note.is_empty() { ui.label(egui::RichText::new(&self.x.prefs_note).color(WARN)); ui.add_space(4.0); }
        let p = &mut self.x.store.prefs;
        ui.label(egui::RichText::new("Editor de áudio (programa que abre uma gravação)").color(FORE2));
        ui.add(egui::TextEdit::singleline(&mut p.audio_editor).hint_text("audacity").desired_width(400.0));
        ui.label(egui::RichText::new("Pasta das gravações (para novos blocos Gravador)").color(FORE2));
        ui.add(egui::TextEdit::singleline(&mut p.default_folder).hint_text(engine::default_recordings_dir()).desired_width(400.0));
        if p.bar_dismissed && ui.small_button("Voltar a avisar sobre programas que faltam").clicked() { p.bar_dismissed = false; }
        ui.separator();
        ui.label(egui::RichText::new("PROGRAMAS DE APOIO").small().color(FORE2));
        for (name, what, found) in [
            ("ffmpeg", "arquivos MP3, AAC, ALAC e FLAC, etiquetas, transmissão e o player", engine::ffmpeg_available()),
            ("ffprobe", "ler etiquetas e duração na janela Gravações", tool_on_path("ffprobe")),
            ("pw-record", "captar o som de um programa só (PipeWire)", tool_on_path("pw-record")),
            ("whisper-cli", "bloco Transcrever", tool_on_path("whisper-cli")),
            ("xdg-open", "abrir em Arquivos e no player de música", tool_on_path("xdg-open")),
            ("gio", "mover gravações para a lixeira", tool_on_path("gio")),
        ] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(if found { "✔" } else { "✘" }).color(if found { GREEN } else { ERR }));
                ui.label(format!("{name} — {what}"));
            });
        }
        ui.separator();
        ui.label(egui::RichText::new(format!("As configurações ficam em {}", store_path().display())).small().color(FORE2));
    }

    fn io_ui(&mut self, ui: &mut egui::Ui) {
        ui.set_width(460.0);
        ui.label("Caminho do arquivo");
        ui.add(egui::TextEdit::singleline(&mut self.x.io_path).hint_text(export_path_for("Minha sessão")).desired_width(440.0));
        ui.label(egui::RichText::new("Importa uma sessão exportada pelo Kalango Audio (.kalango-audio.json).").small().color(FORE2));
        ui.horizontal(|ui| {
            if ui.button("Importar").clicked() {
                let path = self.x.io_path.clone();
                self.x.io_msg = match self.import_session(&path) { Ok(m) => m, Err(e) => format!("Não foi possível importar: {e}") };
            }
            if ui.add_enabled(!self.sessions.is_empty(), egui::Button::new("Exportar a sessão aberta")).clicked() {
                let path = self.x.io_path.clone();
                self.x.io_msg = match self.export_session(&path) { Ok(()) => format!("Salva em {path}"), Err(e) => format!("Não foi possível exportar: {e}") };
            }
        });
        if !self.x.io_msg.is_empty() { ui.label(egui::RichText::new(&self.x.io_msg).color(FORE1)); }
    }

    fn presets_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Ajustes salvos de um bloco, para reusar em qualquer sessão. Aqui você renomeia ou apaga os seus.").small().color(FORE2));
        ui.add_space(4.0);
        let mut apply: Option<usize> = None;
        let mut drop_idx: Option<usize> = None;
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            ui.label(egui::RichText::new("SUAS").small().color(FORE2));
            if self.x.store.presets.is_empty() { ui.label(egui::RichText::new("Nenhuma ainda. Abra os ajustes de um bloco e clique em “Salvar como…”.").small().color(FORE2)); }
            for (i, p) in self.x.store.presets.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(kind_info(&p.kind).map(|k| k.title).unwrap_or("")).small().color(FORE2));
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(150.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() { drop_idx = Some(i); }
                        if ui.small_button("Aplicar ao selecionado").clicked() { apply = Some(i); }
                    });
                });
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new("PRONTAS").small().color(FORE2));
            for p in builtin_presets() {
                ui.label(format!("{}  ·  {}", kind_info(&p.kind).map(|k| k.title).unwrap_or(""), p.name));
            }
        });
        if let Some(i) = drop_idx { self.x.store.presets.remove(i); }
        if let Some(i) = apply {
            let p = self.x.store.presets[i].clone();
            if let Some(sel) = self.sel {
                if let Some(b) = self.sessions[self.cur].blocks.iter_mut().find(|b| b.id == sel && b.kind == p.kind) { apply_preset(b, &p); }
            }
        }
    }

    fn scripts_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(190.0);
                ui.label(egui::RichText::new("SCRIPTS PRONTOS").small().color(FORE2));
                for (i, (n, _)) in script::BUILTIN.iter().enumerate() {
                    if ui.selectable_label(self.x.script_sel == (true, i), *n).clicked() { self.x.script_sel = (true, i); }
                }
                ui.add_space(6.0);
                ui.label(egui::RichText::new("SEUS SCRIPTS").small().color(FORE2));
                for (i, s) in self.x.store.scripts.iter().enumerate() {
                    if ui.selectable_label(self.x.script_sel == (false, i), &s.name).clicked() { self.x.script_sel = (false, i); }
                }
                ui.add_space(6.0);
                if ui.button("+ Novo script").clicked() {
                    self.x.store.scripts.push(Script { name: format!("Script {}", self.x.store.scripts.len() + 1), text: "// O que deve acontecer?\nkalango.log(`evento: ${ctx.kind}`);\n".into() });
                    self.x.script_sel = (false, self.x.store.scripts.len() - 1);
                }
                let dup_label = if self.x.script_sel.0 { "+ Copiar para os seus" } else { "+ Duplicar" };
                if ui.button(dup_label).clicked() {
                    let (name, text) = if self.x.script_sel.0 { let (n, t) = script::BUILTIN[self.x.script_sel.1.min(script::BUILTIN.len() - 1)]; (n.to_string(), t.to_string()) }
                        else if let Some(s) = self.x.store.scripts.get(self.x.script_sel.1) { (s.name.clone(), s.text.clone()) } else { return };
                    self.x.store.scripts.push(Script { name: format!("{name} (cópia)"), text });
                    self.x.script_sel = (false, self.x.store.scripts.len() - 1);
                }
                if ui.add_enabled(!self.x.script_sel.0 && !self.x.store.scripts.is_empty(), egui::Button::new("− Apagar script")).clicked() {
                    let i = self.x.script_sel.1.min(self.x.store.scripts.len().saturating_sub(1));
                    if i < self.x.store.scripts.len() { self.x.store.scripts.remove(i); }
                    self.x.script_sel = (true, 0);
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                let (builtin, idx) = self.x.script_sel;
                let name: String;
                if builtin {
                    let (n, t) = script::BUILTIN[idx.min(script::BUILTIN.len() - 1)];
                    name = n.to_string();
                    ui.label(egui::RichText::new(n).strong());
                    let mut text = t.to_string();
                    ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_rows(10).desired_width(430.0).interactive(false));
                    ui.label(egui::RichText::new("Os scripts prontos não podem ser editados. Copie um para os seus e mude à vontade.").small().color(FORE2));
                } else if let Some(s) = self.x.store.scripts.get_mut(idx) {
                    name = s.name.clone();
                    ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(430.0));
                    ui.add(egui::TextEdit::multiline(&mut s.text).font(egui::TextStyle::Monospace).desired_rows(10).desired_width(430.0));
                } else { ui.label("Nenhum script."); return; }
                ui.horizontal(|ui| {
                    if ui.button("▶ Rodar agora").clicked() { self.run_script_now(&name); }
                    if ui.button("Limpar registro").clicked() { self.x.log.clear(); }
                    if ui.small_button("? Ajuda").clicked() { self.x.show_script_help = true; }
                });
                ui.label(egui::RichText::new("REGISTRO").small().color(FORE2));
                egui::ScrollArea::vertical().max_height(120.0).stick_to_bottom(true).show(ui, |ui| {
                    for l in &self.x.log { ui.label(egui::RichText::new(l).monospace().small()); }
                });
            });
        });
    }
}

/// Column titles of the Sessions window (the first one cannot be hidden).
const HOME_COLUMNS: [&str; 5] = ["Sessão", "Estado", "Início", "Gravações", "Agendamentos"];

/// Where "Exportar sessão…" suggests saving a session: the documents folder, "<name>.kalango-audio.json".
fn export_path_for(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c == '/' || c == '\\' { '-' } else { c }).collect();
    format!("{}/{}.kalango-audio.json", engine::documents_dir(), clean.trim())
}

fn about_ui(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(format!("Kalango Audio {}", env!("CARGO_PKG_VERSION"))).strong().size(18.0));
    ui.label("Parte do KalangoOS. Licença GPL-3.0-or-later.");
    ui.add_space(8.0);
    ui.label(egui::RichText::new("ATALHOS").small().color(FORE2));
    ui.label("Ctrl+N nova sessão · Ctrl+R iniciar/parar · Ctrl+D duplicar sessão · Ctrl+, preferências · Ctrl+Q sair · Delete apaga o bloco selecionado");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_only_touch_their_own_settings() {
        let mut s = Session::new("t");
        let r = s.add_block("recorder", 0.0, 0.0);
        let preset = builtin_presets().into_iter().find(|p| p.name.starts_with("MP3 · máximo")).unwrap();
        let b = s.blocks.iter_mut().find(|b| b.id == r).unwrap();
        b.p.set_text("file_name", "keep me");
        apply_preset(b, &preset);
        assert_eq!(b.p.text("format", ""), "MP3");
        assert_eq!(b.p.num("bitrate", 0.0), 320.0);
        assert_eq!(b.p.text("file_name", ""), "keep me");
    }

    #[test]
    fn our_equalizer_and_compressor_presets_are_well_formed() {
        let eq = eq_presets();
        assert_eq!(eq.len(), EQ_PRESETS_DB.len());
        assert!(eq.iter().all(|(_, v)| v.iter().all(|x| (0.0..=1.0).contains(x))));
        let flat = eq.iter().find(|(n, _)| n == "Plano").unwrap();
        assert!(flat.1.iter().all(|x| *x == 0.5));
        let bass = eq.iter().find(|(n, _)| n == "Mais graves").unwrap();
        assert!((bass.1[0] - 0.75).abs() < 1e-9, "+6 dB is 0.75 on the slider");
        assert!(COMPRESSOR_MODES.iter().all(|m| (0.0..=1.0).contains(&m.1) && (0.0..=1.0).contains(&m.2) && m.3 >= 1.0));
    }

    fn app() -> App {
        App {
            sessions: vec![], cur: 0, sel: None, running: vec![], pan: Vec2::ZERO, drag_wire: None, templates: templates(), show_templates: false,
            devices: vec![], shot: None, frames: 0, last_saved: Instant::now(), saved_snapshot: String::new(), shot_requested: false,
            meters_view: HashMap::new(), disp: HashMap::new(), last_tick: Instant::now(), x: Extras::new(true), timer_started: Default::default(),
            last_timer_check: Instant::now() - std::time::Duration::from_secs(5), apps: Default::default(), want_apps: Default::default(),
        }
    }

    fn chain(app: &mut App) -> (u64, u64, u64) {
        let mut s = Session::new("Chain");
        let a = s.add_block("test_tone", 2.0, 2.0);
        let b = s.add_block("volume", 8.0, 2.0);
        let c = s.add_block("recorder", 14.0, 2.0);
        s.connect(a, b); s.connect(b, c);
        s.blocks.iter_mut().find(|x| x.id == c).unwrap().p.set_text("folder", "/tmp/kalango-audio-test-unused");
        app.sessions.push(s);
        (a, b, c)
    }

    #[test]
    fn automatic_mode_inserts_new_blocks_after_the_selected_one() {
        let mut app = app();
        let (a, b, _c) = chain(&mut app);
        app.sel = Some(a);
        app.add_from_library("balance");
        let s = &app.sessions[0];
        let new = app.sel.unwrap();
        assert_eq!(s.block(a).unwrap().outs, vec![new]);
        assert_eq!(s.block(new).unwrap().outs, vec![b]);
        assert!(s.block(new).unwrap().x > s.block(a).unwrap().x);
        app.sel = Some(a);
        app.add_from_library("system_sound");
        let src = app.sel.unwrap();
        assert!(app.sessions[0].blocks.iter().all(|x| !x.outs.contains(&src)));
    }

    #[test]
    fn manual_mode_leaves_wiring_alone() {
        let mut app = app();
        let (a, b, _) = chain(&mut app);
        app.sessions[0].manual = true;
        app.sel = Some(a);
        app.add_from_library("balance");
        assert_eq!(app.sessions[0].block(a).unwrap().outs, vec![b]);
        assert!(app.sessions[0].block(app.sel.unwrap()).unwrap().outs.is_empty());
    }

    #[test]
    fn removing_a_block_joins_its_neighbours_in_automatic_mode() {
        let mut app = app();
        let (a, b, c) = chain(&mut app);
        app.remove_block_smart(b);
        assert_eq!(app.sessions[0].block(a).unwrap().outs, vec![c]);
        let mut app = self::app();
        let (a, b, _c) = chain(&mut app);
        app.sessions[0].manual = true;
        app.remove_block_smart(b);
        assert!(app.sessions[0].block(a).unwrap().outs.is_empty());
    }

    fn ev(name: &str, uid: Option<u64>) -> ScriptEvent { ScriptEvent { name: name.into(), session: uid, file: None } }

    #[test]
    fn scripts_start_and_stop_sessions_and_switch_blocks() {
        let mut app = app();
        let (_, _, c) = chain(&mut app);
        app.x.store.scripts = vec![Script { name: "T".into(), text: "kalango.log('hi'); kalango.start('chain'); ctx.session.blocks.forEach(b => { if (b.title == 'Gravador') b.bypassed = true; });".into() }];
        let uid = app.sessions[0].uid;
        app.run_script("T", &ev("manual", Some(uid)));
        assert!(app.is_running(uid), "the script started the session");
        assert!(app.x.log.iter().any(|l| l.ends_with("hi")));
        assert!(!app.sessions[0].block(c).unwrap().enabled, "the recorder block was switched off");
        app.x.store.scripts.push(Script { name: "S".into(), text: "kalango.stopAll(); kalango.start('No Such');".into() });
        app.run_script("S", &ev("manual", None));
        assert!(!app.is_running(uid));
        assert!(app.x.log.iter().any(|l| l.contains("nenhuma sessão chamada “No Such”")));
        app.x.store.scripts.push(Script { name: "Bad".into(), text: "this is not (javascript".into() });
        app.run_script("Bad", &ev("manual", None));
        assert!(app.x.log.iter().any(|l| l.contains("parou com um erro")));
        app.run_script("Nope", &ev("manual", None));
        assert!(app.x.log.iter().any(|l| l.contains("nenhum script chamado “Nope”")));
    }

    #[test]
    fn the_quit_script_asks_the_program_to_close() {
        let mut app = app();
        chain(&mut app);
        app.run_script("Parar tudo e sair", &ev("manual", None));
        assert!(app.x.quit_now);
    }

    #[test]
    fn automations_run_scripts_on_the_five_events() {
        let mut app = app();
        chain(&mut app);
        app.x.store.scripts = vec![Script { name: "Hello".into(), text: "kalango.log('hello ' + ctx.kind);".into() }];
        app.sessions[0].automations = EVENTS.iter().map(|(e, _)| Automation { event: e.to_string(), trigger: 0, minutes: 0, script: "Hello".into(), on: true }).collect();
        app.sessions[0].automations.push(Automation { event: "session_starting".into(), trigger: 0, minutes: 0, script: "Hello".into(), on: false });
        let ctx = egui::Context::default();
        let uid = app.sessions[0].uid;
        app.toggle_run(0); // queues session_starting
        app.tick_extras(&ctx);
        let hellos = |a: &App, e: &str| a.x.log.iter().filter(|l| l.ends_with(&format!("hello {e}"))).count();
        assert_eq!(hellos(&app, "session_starting"), 1, "the switched-off automation must not run");
        app.toggle_run(0);
        app.tick_extras(&ctx);
        assert_eq!(hellos(&app, "session_stopped"), 1);
        let before_files = hellos(&app, "recording_finished"); // stopping the session also finished a real file
        app.x.queue_event("timer_starting", Some(uid), None);
        app.x.queue_event("timer_stopped", Some(uid), None);
        app.x.on_files_ended(uid, vec![engine::FileEnd { block: 1, path: "/tmp/x/a.wav".into(), seconds: 2.0, size: 10 }]);
        app.tick_extras(&ctx);
        assert_eq!(hellos(&app, "timer_starting"), 1);
        assert_eq!(hellos(&app, "timer_stopped"), 1);
        assert_eq!(hellos(&app, "recording_finished"), before_files + 1);
        // events of another session do not trigger this session's automations
        app.x.queue_event("session_starting", Some(uid + 1), None);
        app.tick_extras(&ctx);
        assert_eq!(hellos(&app, "session_starting"), 1);
    }

    #[test]
    fn a_finished_recording_reaches_its_script_as_event_file() {
        let mut app = app();
        chain(&mut app);
        app.x.store.scripts = vec![Script { name: "F".into(), text: "kalango.log(ctx.recording.name, ctx.recording.seconds, ctx.session.name);".into() }];
        app.sessions[0].automations = vec![Automation { event: "recording_finished".into(), trigger: 0, minutes: 0, script: "F".into(), on: true }];
        let uid = app.sessions[0].uid;
        app.x.on_files_ended(uid, vec![engine::FileEnd { block: 3, path: "/tmp/x/take 1.wav".into(), seconds: 4.5, size: 99 }]);
        app.tick_extras(&egui::Context::default());
        assert!(app.x.log.iter().any(|l| l.ends_with("take 1.wav 4.5 Chain")), "{:?}", app.x.log);
    }

    #[test]
    fn timer_windows_open_on_their_date_or_weekdays() {
        let (wd, min, today) = (3u32, 600u32, 20261119u32); // a Wednesday, 10:00
        let base = Timer { days: 0, start: 540, minutes: 120, on: true, mode: 1, date: 0, quit_sources: false };
        // Repeat Every: needs the weekday
        assert!(!timer_open(&base, wd, min, today));
        assert!(timer_open(&Timer { days: 1 << 3, ..base.clone() }, wd, min, today));
        assert!(!timer_open(&Timer { days: 1 << 3, start: 700, ..base.clone() }, wd, min, today));
        assert!(!timer_open(&Timer { days: 1 << 3, on: false, ..base.clone() }, wd, min, today));
        // On Date: only that day, whatever the weekday flags say
        assert!(timer_open(&Timer { mode: 0, date: today, ..base.clone() }, wd, min, today));
        assert!(!timer_open(&Timer { mode: 0, date: today + 1, days: 0b1111111, ..base.clone() }, wd, min, today));
        // a window past midnight is still open after midnight the next day
        let late = Timer { days: 1 << 2, start: 23 * 60, minutes: 120, ..base.clone() };
        assert!(timer_open(&late, 2, 23 * 60 + 30, today));
        assert!(timer_open(&late, 3, 30, today));
        assert!(!timer_open(&late, 3, 90, today));
    }

    #[test]
    fn timers_start_and_stop_a_session_and_raise_events() {
        let mut app = app();
        chain(&mut app);
        let (_, min) = engine::local_now();
        let t = Timer { days: 0b1111111, start: min.saturating_sub(1) as u16, minutes: 5, on: true, mode: 1, date: 0, quit_sources: false };
        app.sessions[0].timers = vec![t];
        app.run_timers();
        assert!(app.is_running(app.sessions[0].uid), "timer window is open");
        assert!(app.x.queue.iter().any(|e| e.name == "timer_starting"));
        app.sessions[0].timers[0].on = false;
        app.last_timer_check = Instant::now() - std::time::Duration::from_secs(5);
        app.run_timers();
        assert!(!app.is_running(app.sessions[0].uid), "a session the timer started stops with it");
        assert!(app.x.queue.iter().any(|e| e.name == "timer_stopped"));
        app.toggle_run(0);
        app.last_timer_check = Instant::now() - std::time::Duration::from_secs(5);
        app.run_timers();
        assert!(app.is_running(app.sessions[0].uid), "a session started by hand is never stopped by a timer");
        app.toggle_run(0);
    }

    #[test]
    fn sessions_marked_auto_run_start_when_the_program_opens() {
        let mut app = app();
        chain(&mut app);
        app.sessions[0].auto_run = 1;
        app.x.launched = false;
        app.tick_extras(&egui::Context::default());
        assert!(app.is_running(app.sessions[0].uid));
        app.toggle_run(0);
    }

    #[test]
    fn quitting_asks_first_only_while_recording() {
        let mut app = app();
        chain(&mut app);
        app.request_quit();
        assert!(app.x.quit_now, "nothing running: quit at once");
        app.x.quit_now = false;
        // a running session without a Recorder block does not hold anything open
        let rec = app.sessions[0].blocks.iter().find(|b| b.kind == "recorder").unwrap().id;
        app.sessions[0].blocks.iter_mut().find(|b| b.id == rec).unwrap().enabled = false;
        app.toggle_run(0);
        app.request_quit();
        assert!(app.x.quit_now && app.x.quit_state == 0, "playing without recording: quit at once");
        app.x.quit_now = false;
        app.toggle_run(0);
        app.sessions[0].blocks.iter_mut().find(|b| b.id == rec).unwrap().enabled = true;
        app.toggle_run(0);
        app.request_quit();
        assert_eq!(app.x.quit_state, 1);
        assert!(!app.x.quit_now);
        app.x.quit_state = 2;
        app.quit_dialog(&egui::Context::default());
        assert!(app.x.quit_now && app.running.is_empty(), "sessions are stopped, then the program closes");
    }

    #[test]
    fn sessions_can_be_exported_and_imported_again() {
        let mut app = app();
        chain(&mut app);
        let path = std::env::temp_dir().join(format!("kalango-audio-roundtrip-{}.json", std::process::id()));
        let path = path.to_string_lossy().to_string();
        app.export_session(&path).unwrap();
        let msg = app.import_session(&path).unwrap();
        assert!(msg.contains("Chain"));
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.sessions[1].blocks, app.sessions[0].blocks);
        assert_ne!(app.sessions[1].uid, app.sessions[0].uid);
        assert!(app.import_session("/definitely/not/here.json").is_err());
        std::fs::write(&path, "not a session").unwrap();
        assert!(app.import_session(&path).is_err());
    }

    #[test]
    fn duplicating_and_deleting_sessions() {
        let mut app = app();
        chain(&mut app);
        app.duplicate_session();
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.sessions[1].name, "Chain (cópia)");
        assert_eq!(app.cur, 1);
        app.delete_session();
        assert_eq!(app.sessions.len(), 1);
    }

    fn frame(app: &mut App, ctx: &egui::Context) {
        let mut raw = egui::RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0)));
        let mut out = ctx.run_ui(raw, |ui| {
            app.menu_bar(ui);
            app.notice_bar(ui);
            app.top_bar(ui);
            app.sidebar(ui);
            app.right_panel_tabs(ui);
            app.canvas(ui);
            let c = ui.ctx().clone();
            app.extra_windows(&c);
            app.tick_extras(&c);
        });
        out.textures_delta.clear();
    }

    /// Opens every screen with every kind of block selected, with and without the audio running, to catch crashes.
    #[test]
    fn every_block_and_every_screen_draws_without_crashing() {
        let ctx = egui::Context::default();
        let tmp = std::env::temp_dir().join(format!("kalango-audio-smoke-{}", std::process::id())).to_string_lossy().to_string();
        let mut app = app();
        app.x.show_recordings = true; app.x.show_presets = true; app.x.show_scripts = true; app.x.show_mini = true; app.x.show_home = true;
        app.x.show_prefs = true; app.x.show_about = true; app.x.show_io = true; app.x.show_script_help = true; app.show_templates = true;
        for kind in KINDS.iter().map(|k| k.id) {
            let mut s = Session::new(&format!("Smoke {kind}"));
            let tone = s.add_block("test_tone", 2.0, 2.0);
            let other = s.add_block("test_tone", 2.0, 8.0);
            let b = s.add_block(kind, 8.0, 2.0);
            let rec = s.add_block("recorder", 14.0, 2.0);
            if !is_source(kind) { s.connect(tone, b); s.connect(other, b); s.connect(b, rec); }
            for x in s.blocks.iter_mut() { if x.id == rec { x.p.set_text("folder", &tmp); } }
            app.sessions = vec![s];
            app.cur = 0;
            app.sel = Some(b);
            let runs = !matches!(kind, "device_input" | "system_sound" | "app_source" | "device_output");
            for tab in 0..6 {
                app.x.right_tab = tab;
                frame(&mut app, &ctx);
            }
            if runs {
                app.toggle_run(0);
                for _ in 0..6 { frame(&mut app, &ctx); std::thread::sleep(std::time::Duration::from_millis(25)); }
                app.sessions[0].blocks.iter_mut().find(|x| x.id == b).unwrap().enabled = false;
                for _ in 0..3 { frame(&mut app, &ctx); std::thread::sleep(std::time::Duration::from_millis(20)); }
                app.toggle_run(0);
            }
        }
        for t in templates() {
            app.sessions = vec![t.instantiate()];
            app.sel = app.sessions[0].blocks.first().map(|b| b.id);
            app.x.right_tab = 1;
            frame(&mut app, &ctx);
        }
        app.sessions.clear();
        app.sel = None;
        frame(&mut app, &ctx);
        let _ = std::fs::read_dir(&tmp).map(|d| d.count());
    }

    #[test]
    fn old_saved_sessions_still_open_and_damaged_files_do_not_crash() {
        // a session saved by an early version, without timers, automations, manual mode, notes or id
        let old = r#"[{"name":"Old","blocks":[{"id":1,"kind":"test_tone","x":2.0,"y":2.0,"enabled":true,"outs":[],"p":{"n":{"freq":440.0},"s":{}}}],"next_id":2,"timers":[{"days":62,"start":540,"minutes":60,"on":true}],"automations":[{"trigger":1,"minutes":5,"script":"X","on":true}]}]"#;
        let mut v: Vec<Session> = serde_json::from_str(old).unwrap();
        assert!(!v[0].manual && v[0].notes.is_empty() && v[0].auto_run == 0);
        assert_eq!(v[0].timers[0].mode, 1, "old timers repeat on weekdays");
        for a in v[0].automations.iter_mut() { a.migrate(); }
        assert_eq!(v[0].automations[0].event, "session_stopped");
        assert!(serde_json::from_str::<Vec<Session>>("{ broken").is_err());
        assert!(serde_json::from_str::<Store>("{ broken").is_err());
        let s: Store = serde_json::from_str("{}").unwrap_or_default();
        assert!(s.scripts.is_empty() && s.presets.is_empty());
    }

    #[test]
    fn edits_can_be_undone_and_redone() {
        let mut app = app();
        let (_, b, _) = chain(&mut app);
        app.track_undo(); // baseline
        let gain = |a: &App| a.sessions[0].block(b).unwrap().p.num("gain", 9.0);
        let set = |a: &mut App, v: f64| { a.sessions[0].blocks.iter_mut().find(|x| x.id == b).unwrap().p.set("gain", v); };
        let settle = |a: &mut App| { a.track_undo(); a.x.last_change = Instant::now() - std::time::Duration::from_secs(2); a.track_undo(); };
        set(&mut app, 0.5);
        settle(&mut app);
        set(&mut app, 0.25);
        settle(&mut app);
        assert_eq!(gain(&app), 0.25);
        app.undo();
        assert_eq!(gain(&app), 0.5);
        app.undo();
        assert_eq!(gain(&app), 1.0);
        app.undo(); // nothing left: stays put
        assert_eq!(gain(&app), 1.0);
        app.redo();
        assert_eq!(gain(&app), 0.5);
        app.redo();
        assert_eq!(gain(&app), 0.25);
        // a new edit clears what could be redone
        app.undo();
        set(&mut app, 0.9);
        settle(&mut app);
        assert!(app.x.redo.is_empty());
        // an edit that has not settled yet is still undone in one step
        set(&mut app, 0.1);
        app.undo();
        assert_eq!(gain(&app), 0.9);
        // undo keeps the session's identity
        assert_eq!(app.sessions.len(), 1);
    }
}
