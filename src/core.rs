//! The app without its window: sessions, the running engines, timers, automations and scripts, undo, recordings on
//! disk and saving. The GTK interface (`gui`) reads and changes it; the tests drive it directly.
use crate::audio;
use crate::engine::{self, Engine};
use crate::i18n::{tr, trf};
use crate::media;
use crate::meter::Disp;
use crate::model::*;
use crate::presets::{Preset, Store};
use crate::script;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct Running {
    pub uid: u64,
    pub engine: Engine,
    pub since: Instant,
}

/// Something that happened that an automation can react to.
#[derive(Clone)]
pub struct ScriptEvent {
    pub name: String,
    pub session: Option<u64>,
    pub file: Option<script::RecordingInfo>,
}

/// A recording found on disk.
#[derive(Clone, Debug)]
pub struct RecFile {
    pub path: PathBuf,
    pub name: String,
    pub folder: String,
    pub size: u64,
    /// seconds since the epoch
    pub modified: u64,
    pub dur: Option<f64>,
}

impl RecFile {
    /// The transcript written next to it (`<file>.txt` or `<stem>.txt`), if any.
    pub fn transcript(&self) -> Option<PathBuf> {
        let a = PathBuf::from(format!("{}.txt", self.path.display()));
        let b = self.path.with_extension("txt");
        [a, b].into_iter().find(|p| p.is_file())
    }
}

/// What a timer says about "now": is its window open?
pub fn timer_open(t: &Timer, wd: u32, min: u32, today: u32) -> bool {
    if !t.on {
        return false;
    }
    let end = t.start as u32 + t.minutes as u32;
    if t.mode == 0 {
        // on a date: one day only (a window that runs past midnight is not carried into the next day)
        return t.date == today && min >= t.start as u32 && min < end;
    }
    let in_today = t.days & (1 << wd) != 0 && min >= t.start as u32 && min < end;
    // a window that runs past midnight is still open after midnight, on the next day
    let prev = (wd + 6) % 7;
    let spill = t.days & (1 << prev) != 0 && end > 1440 && min < end - 1440;
    in_today || spill
}

pub fn expand_home(p: &str) -> PathBuf {
    if let Some(r) = p.strip_prefix('~') {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(r.trim_start_matches('/'));
    }
    PathBuf::from(p)
}

pub fn now_hms() -> String {
    let (_, m) = engine::local_now();
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() % 60).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", m / 60, m % 60, s)
}

/// "00:42:13"
pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// "48:02" or "1:12:08"
pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "312 MB", "0,4 MB" (comma in pt_BR)
pub fn fmt_size(b: u64) -> String {
    let mb = b as f64 / 1_048_576.0;
    let s = if mb >= 100.0 { format!("{mb:.0} MB") } else if mb >= 0.1 { format!("{mb:.1} MB") } else { format!("{} KB", (b / 1024).max(1)) };
    if crate::i18n::system_locale().is_some() { s.replace('.', ",") } else { s }
}

/// The engine reports states with short English codes ("recording", "error: …"); this is what the person reads.
pub fn state_label(state: &str) -> String {
    if let Some(e) = state.strip_prefix("error: ").or_else(|| state.strip_prefix("error:")) {
        return trf("error: {what}", &[("what", e.trim().to_string())]);
    }
    match state {
        "recording" => tr("recording").into(),
        "paused" => tr("paused").into(),
        "silent" => tr("silent").into(),
        "finished" => tr("finished").into(),
        "streaming" => tr("streaming").into(),
        "listening" => tr("listening").into(),
        "running" => tr("running").into(),
        "error" => tr("error").into(),
        s => s.to_string(),
    }
}

fn store_path() -> PathBuf {
    config_path().with_file_name("extras.json")
}

pub struct Core {
    pub sessions: Vec<Session>,
    /// the session open in the editor
    pub cur: usize,
    /// the selected block in the editor
    pub sel: Option<u64>,
    pub running: Vec<Running>,
    pub templates: Vec<Template>,
    pub devices: Vec<(String, String)>,
    pub store: Store,
    pub recs: Vec<RecFile>,
    pub rec_scanned: Option<Instant>,
    /// the script log (newest last)
    pub log: Vec<String>,
    /// messages a script asked to show when no desktop notification could be sent
    pub dialogs: Vec<String>,
    pub queue: VecDeque<ScriptEvent>,
    pub timer_state: HashMap<(u64, usize), bool>,
    pub timer_started: HashSet<u64>,
    pub last_timer_check: Instant,
    pub launched: bool,
    /// 0 = normal, 1 = asking whether to stop recordings and quit
    pub quit_state: u8,
    /// set when the app should close now (sessions already stopped)
    pub quit_now: bool,
    /// a script asked for the audio editor but none is set: the settings should open
    pub wants_editor_setting: bool,
    /// live meters per (session uid, block id)
    pub disp: HashMap<(u64, u64), Disp>,
    last_meter_tick: Instant,
    last_dev: Instant,
    undo: Vec<(u64, String)>,
    pub redo: Vec<(u64, String)>,
    base: Option<(u64, String)>,
    last_seen: String,
    pub last_change: Instant,
    /// false in tests and screenshots: nothing is written to disk
    persist: bool,
    saved_sessions: String,
    saved_store: String,
    last_save: Instant,
    /// the programs playing sound right now (kept fresh by a helper thread while `want_apps` is set)
    pub apps: Arc<Mutex<Vec<audio::AppInfo>>>,
    pub want_apps: Arc<AtomicBool>,
}

impl Core {
    /// The app as the person left it (sessions and settings from ~/.config/kalango/audio).
    pub fn load() -> Core {
        let sessions = load_sessions();
        let store: Store = std::fs::read_to_string(store_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut c = Core::with(sessions, store, true);
        c.watch_apps();
        c.devices = audio::list_devices();
        c
    }

    /// Keeps `apps` (the programs playing sound) fresh in a helper thread while `want_apps` is set.
    pub fn watch_apps(&self) {
        let (a, w) = (self.apps.clone(), self.want_apps.clone());
        std::thread::spawn(move || loop {
            if w.load(Ordering::Relaxed) {
                let now = audio::list_apps();
                *a.lock().unwrap() = now;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        });
    }

    /// An app that keeps nothing on disk (tests, screenshots).
    pub fn ephemeral(sessions: Vec<Session>) -> Core {
        Core::with(sessions, Store::default(), false)
    }

    fn with(sessions: Vec<Session>, mut store: Store, persist: bool) -> Core {
        if store.prefs.default_folder.is_empty() {
            store.prefs.default_folder = engine::default_recordings_dir();
        }
        let saved_sessions = serde_json::to_string(&sessions).unwrap_or_default();
        let saved_store = serde_json::to_string(&store).unwrap_or_default();
        Core {
            sessions,
            cur: 0,
            sel: None,
            running: vec![],
            templates: templates(),
            devices: vec![],
            store,
            recs: vec![],
            rec_scanned: None,
            log: vec![],
            dialogs: vec![],
            queue: VecDeque::new(),
            timer_state: HashMap::new(),
            timer_started: HashSet::new(),
            last_timer_check: Instant::now() - std::time::Duration::from_secs(5),
            launched: !persist,
            quit_state: 0,
            quit_now: false,
            wants_editor_setting: false,
            disp: HashMap::new(),
            last_meter_tick: Instant::now(),
            last_dev: Instant::now(),
            undo: vec![],
            redo: vec![],
            base: None,
            last_seen: String::new(),
            last_change: Instant::now(),
            persist,
            saved_sessions,
            saved_store,
            last_save: Instant::now(),
            apps: Default::default(),
            want_apps: Default::default(),
        }
    }

    // ---------------- sessions ----------------

    pub fn session(&self) -> Option<&Session> {
        self.sessions.get(self.cur)
    }

    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.sessions.get_mut(self.cur)
    }

    pub fn index_of(&self, uid: u64) -> Option<usize> {
        self.sessions.iter().position(|s| s.uid == uid)
    }

    pub fn find_session(&self, name: &str) -> Option<usize> {
        self.sessions.iter().position(|s| s.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Opens a session in the editor.
    pub fn open(&mut self, idx: usize) {
        if idx < self.sessions.len() {
            self.cur = idx;
            self.sel = None;
        }
    }

    /// A new session from a template (by index), opened; returns its index.
    pub fn new_from_template(&mut self, t: usize) -> usize {
        let mut s = self.templates.get(t).map(|t| t.instantiate()).unwrap_or_else(|| Session::new(tr("New session")));
        let format = self.store.prefs.default_format.clone();
        for b in s.blocks.iter_mut().filter(|b| b.kind == "recorder") {
            if !format.is_empty() {
                b.p.set_text("format", &format);
            }
        }
        // a name already taken gets a number
        let base = s.name.clone();
        let mut n = 2;
        while self.sessions.iter().any(|x| x.name == s.name) {
            s.name = format!("{base} {n}");
            n += 1;
        }
        self.sessions.push(s);
        self.cur = self.sessions.len() - 1;
        self.sel = None;
        self.cur
    }

    pub fn duplicate_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let mut s = self.sessions[self.cur].clone();
        s.uid = new_uid();
        s.name = trf("{name} (copy)", &[("name", s.name.clone())]);
        self.sessions.insert(self.cur + 1, s);
        self.cur += 1;
        self.sel = None;
    }

    pub fn delete_session(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let uid = self.sessions[self.cur].uid;
        self.running.retain(|r| r.uid != uid);
        self.sessions.remove(self.cur);
        self.cur = self.cur.min(self.sessions.len().saturating_sub(1));
        self.sel = None;
    }

    /// Removes a block; in automatic mode the blocks around it are joined together.
    pub fn remove_block_smart(&mut self, id: u64) {
        let Some(s) = self.sessions.get_mut(self.cur) else { return };
        if !s.manual {
            let outs = s.block(id).map(|b| b.outs.clone()).unwrap_or_default();
            let ups: Vec<u64> = s.blocks.iter().filter(|b| b.outs.contains(&id)).map(|b| b.id).collect();
            for u in ups {
                for o in &outs {
                    s.connect(u, *o);
                }
            }
        }
        s.remove_block(id);
        if self.sel == Some(id) {
            self.sel = None;
        }
    }

    /// Adds a block from the library. Without a place it goes after the selected block (automatic mode) or in the
    /// first free spot; `at` puts it there (grid units). Returns its id.
    pub fn add_from_library(&mut self, kind: &str, at: Option<(f32, f32)>) -> Option<u64> {
        let default_folder = self.store.prefs.default_folder.clone();
        let default_format = self.store.prefs.default_format.clone();
        let sel = self.sel;
        let s = self.sessions.get_mut(self.cur)?;
        let manual = s.manual;
        let after = sel.and_then(|i| s.block(i)).map(|b| (b.id, b.x, b.y, b.outs.clone()));
        let (x, y) = match (at, &after, manual, is_source(kind)) {
            (Some(p), _, _, _) => p,
            (None, Some((_, bx, by, _)), false, false) => (*bx + 6.0, *by),
            _ => {
                let (mut x, mut y) = (2.0, 2.0);
                while s.blocks.iter().any(|b| (b.x - x).abs() < 4.0 && (b.y - y).abs() < 3.5) {
                    x += 6.0;
                    if x > 30.0 {
                        x = 2.0;
                        y += 5.0;
                    }
                }
                (x, y)
            }
        };
        let id = s.add_block(kind, x, y);
        if kind == "recorder" {
            if let Some(b) = s.blocks.iter_mut().find(|b| b.id == id) {
                b.p.set_text("folder", &default_folder);
                if !default_format.is_empty() {
                    b.p.set_text("format", &default_format);
                }
            }
        }
        // automatic mode: the new block goes right after the selected one
        if let (None, Some((aid, _, _, outs)), false, false) = (at, after, manual, is_source(kind)) {
            let a_is_end = matches!(s.block(aid).map(|b| b.kind.as_str()), Some("device_output") | Some("radio_stream") | Some("rtmp_stream"));
            if !a_is_end {
                for o in &outs {
                    s.disconnect(aid, *o);
                    s.connect(id, *o);
                }
                s.connect(aid, id);
            }
        }
        self.sel = Some(id);
        Some(id)
    }

    /// Puts a block in the middle of a wire (dropped on it): from → new → to.
    pub fn insert_on_wire(&mut self, kind: &str, from: u64, to: u64, at: (f32, f32)) -> Option<u64> {
        let id = self.add_from_library(kind, Some(at))?;
        let s = self.sessions.get_mut(self.cur)?;
        s.disconnect(from, to);
        s.connect(from, id);
        s.connect(id, to);
        Some(id)
    }

    pub fn export_session(&mut self, path: &str) -> Result<(), String> {
        let s = self.sessions.get(self.cur).ok_or_else(|| tr("No session is open").to_string())?;
        let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
        let p = expand_home(path);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::write(&p, text).map_err(|e| e.to_string())
    }

    pub fn import_session(&mut self, path: &str) -> Result<String, String> {
        let p = expand_home(path);
        let text = std::fs::read(&p).map_err(|e| trf("Could not read the file: {err}", &[("err", e.to_string())]))?;
        let mut s = serde_json::from_slice::<Session>(&text).map_err(|e| trf("Not a Kalango Audio session: {err}", &[("err", e.to_string())]))?;
        s.uid = new_uid();
        for a in s.automations.iter_mut() {
            a.migrate();
        }
        let name = s.name.clone();
        self.sessions.push(s);
        self.cur = self.sessions.len() - 1;
        self.sel = None;
        Ok(trf("Session “{name}” imported", &[("name", name)]))
    }

    // ---------------- running ----------------

    pub fn is_running(&self, uid: u64) -> bool {
        self.running.iter().any(|r| r.uid == uid)
    }

    pub fn running_of(&self, uid: u64) -> Option<&Running> {
        self.running.iter().find(|r| r.uid == uid)
    }

    /// Seconds since the session started (None when it is not running).
    pub fn running_for(&self, uid: u64) -> Option<f64> {
        self.running_of(uid).map(|r| r.since.elapsed().as_secs_f64())
    }

    /// Recorder status of a block in a running session.
    pub fn rec_status(&self, uid: u64, block: u64) -> Option<engine::RecStatus> {
        self.running_of(uid).and_then(|r| r.engine.shared.rec.lock().unwrap().get(&block).cloned())
    }

    /// The error a block reports while running (no device, no permission…).
    pub fn block_error(&self, uid: u64, block: u64) -> Option<String> {
        self.running_of(uid).and_then(|r| r.engine.shared.errors.lock().unwrap().get(&block).cloned())
    }

    /// How busy the audio thread of a session is (0..1), and how many chunks ran late.
    pub fn load_of(&self, uid: u64) -> Option<(f32, u32)> {
        self.running_of(uid).map(|r| {
            (r.engine.shared.load.load(Ordering::Relaxed) as f32 / 1000.0, r.engine.shared.overruns.load(Ordering::Relaxed))
        })
    }

    /// Whether any Recorder of the session is writing now.
    pub fn is_recording(&self, uid: u64) -> bool {
        let Some(r) = self.running_of(uid) else { return false };
        let st = r.engine.shared.rec.lock().unwrap();
        st.values().any(|x| x.state == "recording")
    }

    pub fn toggle_run(&mut self, idx: usize) {
        let Some(s) = self.sessions.get(idx) else { return };
        let uid = s.uid;
        if let Some(p) = self.running.iter().position(|r| r.uid == uid) {
            let r = self.running.remove(p);
            let shared = r.engine.shared.clone();
            drop(r); // stopping waits until the files are finished
            let ended: Vec<engine::FileEnd> = std::mem::take(&mut *shared.events.lock().unwrap());
            self.on_files_ended(uid, ended);
            self.queue_event("session_stopped", Some(uid), None);
            self.disp.retain(|(s, _), _| *s != uid);
            self.rec_scanned = None;
        } else {
            self.queue_event("session_starting", Some(uid), None);
            let s = &self.sessions[idx];
            // "open an address": open the chosen program with the address when the session starts
            for b in s.blocks.iter().filter(|b| b.kind == "app_source" && b.enabled && b.p.num("url_on", 0.0) > 0.5) {
                audio::launch_app(&b.p.text("app", ""), &b.p.text("url", ""));
            }
            self.running.push(Running { uid, engine: Engine::start_with_rate(s.blocks.clone(), &s.name, s.sample_rate), since: Instant::now() });
        }
    }

    pub fn stop_all(&mut self) {
        for i in 0..self.sessions.len() {
            if self.is_running(self.sessions[i].uid) {
                self.toggle_run(i);
            }
        }
    }

    /// How many Recorder blocks are writing (or holding open) a file right now, across the running sessions.
    pub fn recordings_running(&self) -> usize {
        self.running
            .iter()
            .map(|r| {
                let Some(s) = self.sessions.iter().find(|s| s.uid == r.uid) else { return 0 };
                let st = r.engine.shared.rec.lock().unwrap().clone();
                s.blocks
                    .iter()
                    .filter(|b| b.kind == "recorder" && b.enabled)
                    .filter(|b| st.get(&b.id).map(|x| x.state != "finished" && !x.state.starts_with("error")).unwrap_or(true))
                    .count()
            })
            .sum()
    }

    /// Quitting while something is being recorded asks first, so files are closed properly; otherwise the app quits.
    pub fn request_quit(&mut self) {
        if self.recordings_running() == 0 {
            self.stop_all();
            self.quit_now = true;
        } else {
            self.quit_state = 1;
        }
    }

    /// The answer to "stop the recordings and quit?".
    pub fn confirm_quit(&mut self, yes: bool) {
        self.quit_state = 0;
        if yes {
            self.stop_all();
            self.quit_now = true;
        }
    }

    /// Quits the programs a session's Application blocks are listening to (timer option "quit sources when done").
    pub fn quit_sources(&mut self, idx: usize) {
        let names: Vec<String> = self.sessions[idx]
            .blocks
            .iter()
            .filter(|b| b.kind == "app_source")
            .map(|b| b.p.text("app", ""))
            .filter(|n| !n.is_empty() && n != "Web Browser" && n != "Any Application")
            .collect();
        for a in audio::list_apps() {
            if a.pid > 1 && names.iter().any(|n| audio::app_matches(n, &a)) {
                unsafe {
                    libc::kill(a.pid, libc::SIGTERM);
                }
            }
        }
    }

    /// Timers start a session when their window opens and stop it when it closes, and raise the timer events.
    pub fn run_timers(&mut self) {
        if self.last_timer_check.elapsed().as_secs() < 1 {
            return;
        }
        self.last_timer_check = Instant::now();
        let (wd, min) = engine::local_now();
        let today = engine::local_date();
        for i in 0..self.sessions.len() {
            let uid = self.sessions[i].uid;
            let mut any_open = false;
            let mut edges: Vec<(usize, bool)> = vec![];
            for (ti, t) in self.sessions[i].timers.iter().enumerate() {
                let open = timer_open(t, wd, min, today);
                let was = *self.timer_state.get(&(uid, ti)).unwrap_or(&false);
                if open != was {
                    edges.push((ti, open));
                }
                any_open |= open;
            }
            for (ti, open) in edges {
                self.timer_state.insert((uid, ti), open);
                self.queue_event(if open { "timer_starting" } else { "timer_stopped" }, Some(uid), None);
                if !open && self.sessions[i].timers[ti].quit_sources {
                    self.quit_sources(i);
                }
            }
            let running = self.is_running(uid);
            if any_open && !running {
                self.toggle_run(i);
                self.timer_started.insert(uid);
            } else if !any_open && running && self.timer_started.contains(&uid) {
                self.toggle_run(i);
                self.timer_started.remove(&uid);
            } else if !running {
                self.timer_started.remove(&uid);
            }
        }
    }

    // ---------------- events and scripts ----------------

    pub fn queue_event(&mut self, name: &str, session: Option<u64>, file: Option<script::RecordingInfo>) {
        self.queue.push_back(ScriptEvent { name: name.to_string(), session, file });
    }

    /// Files that a session has finished writing become "recording finished" events.
    pub fn on_files_ended(&mut self, uid: u64, ends: Vec<engine::FileEnd>) {
        for f in ends {
            let name = Path::new(&f.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.queue_event("recording_finished", Some(uid), Some(script::RecordingInfo { path: f.path, name, seconds: f.seconds, bytes: f.size }));
        }
    }

    pub fn script_text(&self, name: &str) -> Option<String> {
        script::BUILTIN
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| t.to_string())
            .or_else(|| self.store.scripts.iter().find(|s| s.name == name).map(|s| s.text.clone()))
    }

    fn script_input(&self, ev: &ScriptEvent) -> script::Input {
        let sessions = self
            .sessions
            .iter()
            .map(|s| script::SessionInfo {
                name: s.name.clone(),
                running: self.is_running(s.uid),
                blocks: s
                    .blocks
                    .iter()
                    .map(|b| script::BlockInfo { id: b.id, title: kind_info(&b.kind).map(|k| tr(k.title).to_string()).unwrap_or_default(), bypassed: !b.enabled })
                    .collect(),
            })
            .collect();
        script::Input {
            sessions,
            kind: ev.name.clone(),
            session: ev.session.and_then(|u| self.sessions.iter().find(|s| s.uid == u)).map(|s| s.name.clone()),
            recording: ev.file.clone(),
        }
    }

    /// Runs a script by name for an event and carries out what it asked for.
    pub fn run_script(&mut self, name: &str, ev: &ScriptEvent) {
        let stamp = now_hms();
        let Some(text) = self.script_text(name) else {
            self.log.push(format!("{stamp}  {}", trf("no script called “{name}”", &[("name", name.to_string())])));
            return;
        };
        let out = script::run(&text, &self.script_input(ev));
        if let Some(e) = &out.err {
            self.log.push(format!("{stamp}  {}", trf("“{name}” stopped with an error: {err}", &[("name", name.to_string()), ("err", e.clone())])));
        }
        for a in &out.acts {
            let arg = a.get(1).cloned().unwrap_or_default();
            match a[0].as_str() {
                "log" => self.log.push(format!("{stamp}  {arg}")),
                "notify" => {
                    self.log.push(format!("{stamp}  {arg}"));
                    if !script::desktop_notify(&arg) {
                        self.dialogs.push(arg);
                    }
                }
                "start_session" | "stop_session" => match self.find_session(&arg) {
                    Some(i) => {
                        let want = a[0] == "start_session";
                        if want != self.is_running(self.sessions[i].uid) {
                            self.toggle_run(i);
                        }
                    }
                    None => self.log.push(format!("{stamp}  {}", trf("no session called “{name}”", &[("name", arg)]))),
                },
                "stop_all" => self.stop_all(),
                "quit" => {
                    self.stop_all();
                    self.quit_now = true;
                }
                "focus" => script::request_focus(),
                "open_session" => {
                    if let Some(i) = self.find_session(&arg) {
                        self.open(i);
                    }
                }
                "trash" => {
                    if !media::trash(Path::new(&arg)) {
                        self.log.push(format!("{stamp}  {}", trf("could not move “{path}” to the trash", &[("path", arg)])));
                    }
                    self.rec_scanned = None;
                }
                "show_in_files" => {
                    if let Some(d) = Path::new(&arg).parent() {
                        let _ = std::process::Command::new("xdg-open").arg(d).spawn();
                    }
                }
                "open_player" => {
                    let _ = std::process::Command::new("xdg-open").arg(&arg).spawn();
                }
                "transcribe" => {
                    // model and language from the script, else from a Transcribe block (the event's session first)
                    let blocks: Vec<&Block> = ev
                        .session
                        .and_then(|u| self.sessions.iter().find(|s| s.uid == u))
                        .into_iter()
                        .chain(self.sessions.iter())
                        .flat_map(|s| s.blocks.iter())
                        .filter(|b| b.kind == "transcribe")
                        .collect();
                    let pick = |k: &str, given: Option<&String>| {
                        given.filter(|v| !v.is_empty()).cloned().or_else(|| blocks.iter().map(|b| b.p.text(k, "")).find(|v| !v.is_empty())).unwrap_or_default()
                    };
                    let (model, language, command) = (pick("model", a.get(2)), pick("language", a.get(3)), pick("command", None));
                    match script::transcribe_file(&arg, &command, &model, &language) {
                        Ok(txt) => self.log.push(format!("{stamp}  {}", trf("transcribing to “{path}”", &[("path", txt.display().to_string())]))),
                        Err(e) => self.log.push(format!("{stamp}  {}", trf("could not transcribe: {err}", &[("err", e)]))),
                    }
                }
                "open_editor" => {
                    let ed = self.store.prefs.audio_editor.trim().to_string();
                    if ed.is_empty() {
                        self.wants_editor_setting = true;
                    } else {
                        let _ = std::process::Command::new(ed).arg(&arg).spawn();
                    }
                }
                _ => {}
            }
        }
        for (sname, id, disabled) in &out.blocks {
            if let Some(i) = self.find_session(sname) {
                if let Some(b) = self.sessions[i].blocks.iter_mut().find(|b| b.id == *id) {
                    b.enabled = !*disabled;
                }
            }
        }
    }

    /// "Run now": scripts that need a file or a session get the latest recording / the open session.
    pub fn run_script_now(&mut self, name: &str) {
        let text = self.script_text(name).unwrap_or_default();
        let uid = self.sessions.get(self.cur).map(|s| s.uid);
        let needs = script::needs(&text);
        let mut ev = ScriptEvent { name: "manual".into(), session: if needs.session { uid } else { None }, file: None };
        if needs.recording {
            if self.rec_scanned.is_none() {
                self.scan_recordings();
            }
            if let Some(r) = self.recs.first() {
                ev.file = Some(script::RecordingInfo {
                    path: r.path.to_string_lossy().to_string(),
                    name: r.name.clone(),
                    seconds: r.dur.or_else(|| media::duration(&r.path)).unwrap_or(0.0),
                    bytes: r.size,
                });
            } else {
                self.log.push(format!("{}  {}", now_hms(), trf("“{name}” needs a recording, and there is none yet", &[("name", name.to_string())])));
                return;
            }
        }
        self.run_script(name, &ev);
    }

    pub fn fire_events(&mut self) {
        let mut guard = 0;
        while let Some(ev) = self.queue.pop_front() {
            guard += 1;
            if guard > 100 {
                self.queue.clear();
                break;
            }
            let mut todo: Vec<String> = vec![];
            for s in &self.sessions {
                if ev.session.is_some() && ev.session != Some(s.uid) {
                    continue;
                }
                for a in &s.automations {
                    if a.on && a.event == ev.name && !a.script.is_empty() {
                        todo.push(a.script.clone());
                    }
                }
            }
            for name in todo {
                let label = EVENTS.iter().find(|e| e.0 == ev.name).map(|e| tr(e.1)).unwrap_or(&ev.name).to_string();
                self.log.push(format!("{}  {label} → {}", now_hms(), trf("running “{name}”", &[("name", name.clone())])));
                self.run_script(&name, &ev);
            }
        }
    }

    // ---------------- undo and redo ----------------

    /// Watches the open session; when its settings stop changing for half a second that state becomes one undo step.
    pub fn track_undo(&mut self) {
        let Some(s) = self.sessions.get(self.cur) else { return };
        let (uid, snap) = (s.uid, serde_json::to_string(s).unwrap_or_default());
        match &self.base {
            Some((u, _)) if *u == uid => {}
            _ => {
                self.base = Some((uid, snap.clone()));
                self.last_seen = snap;
                return;
            }
        }
        if snap != self.last_seen {
            self.last_seen = snap.clone();
            self.last_change = Instant::now();
        }
        if self.base.as_ref().map(|b| b.1 != snap).unwrap_or(false) && self.last_change.elapsed().as_millis() > 500 {
            self.commit_undo(snap);
        }
    }

    fn commit_undo(&mut self, snap: String) {
        if let Some(b) = self.base.take() {
            self.undo.push(b);
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.base = Some((self.sessions[self.cur].uid, snap));
        }
    }

    fn settle_undo(&mut self) {
        if let Some(s) = self.sessions.get(self.cur) {
            let snap = serde_json::to_string(s).unwrap_or_default();
            if self.base.as_ref().map(|b| b.0 == s.uid && b.1 != snap).unwrap_or(false) {
                self.commit_undo(snap);
            }
        }
    }

    fn restore(&mut self, uid: u64, state: &str) {
        let Some(i) = self.index_of(uid) else { return };
        if let Ok(mut s) = serde_json::from_str::<Session>(state) {
            s.uid = uid;
            self.sessions[i] = s;
            self.cur = i;
            self.sel = None;
            self.base = Some((uid, state.to_string()));
            self.last_seen = state.to_string();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn undo(&mut self) {
        self.settle_undo();
        if let (Some((uid, state)), Some(cur)) = (self.undo.pop(), self.base.clone()) {
            self.redo.push(cur);
            self.restore(uid, &state);
        }
    }

    pub fn redo(&mut self) {
        self.settle_undo();
        if let (Some((uid, state)), Some(cur)) = (self.redo.pop(), self.base.clone()) {
            self.undo.push(cur);
            self.restore(uid, &state);
        }
    }

    // ---------------- recordings ----------------

    fn session_folders(&self, idx: usize) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.sessions[idx].blocks.iter().filter(|b| b.kind == "recorder").map(|b| expand_home(&b.p.text("folder", ""))).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The folders the recordings live in: the default one and every Recorder's.
    pub fn recording_folders(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = vec![expand_home(&self.store.prefs.default_folder)];
        for i in 0..self.sessions.len() {
            folders.extend(self.session_folders(i));
        }
        folders.sort();
        folders.dedup();
        folders
    }

    pub fn scan_recordings(&mut self) {
        let mut v: Vec<RecFile> = vec![];
        for f in self.recording_folders() {
            let Ok(rd) = std::fs::read_dir(&f) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
                if !["wav", "aiff", "aif", "mp3", "m4a", "mp4", "flac"].contains(&ext.as_str()) {
                    continue;
                }
                let Ok(md) = e.metadata() else { continue };
                let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                let dur = if ext == "wav" { hound::WavReader::open(&p).ok().map(|r| r.duration() as f64 / r.spec().sample_rate as f64) } else { None };
                v.push(RecFile {
                    name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    folder: f.to_string_lossy().to_string(),
                    path: p,
                    size: md.len(),
                    modified,
                    dur,
                });
            }
        }
        v.sort_by(|a, b| b.modified.cmp(&a.modified));
        v.truncate(500);
        self.recs = v;
        self.rec_scanned = Some(Instant::now());
    }

    /// The session a recording came from (by its folder and name), if it can tell.
    pub fn session_of_recording(&self, r: &RecFile) -> Option<&Session> {
        self.sessions.iter().find(|s| r.name.starts_with(&s.name)).or_else(|| {
            self.sessions.iter().find(|s| s.blocks.iter().any(|b| b.kind == "recorder" && expand_home(&b.p.text("folder", "")) == Path::new(&r.folder)))
        })
    }

    pub fn tags_of(&self, path: &Path) -> Vec<String> {
        self.store.tags.get(&path.to_string_lossy().to_string()).cloned().unwrap_or_default()
    }

    pub fn set_tags(&mut self, path: &Path, tags: Vec<String>) {
        let k = path.to_string_lossy().to_string();
        if tags.is_empty() {
            self.store.tags.remove(&k);
        } else {
            self.store.tags.insert(k, tags);
        }
    }

    /// Moves a recording to the trash (and its transcript with it).
    pub fn trash_recording(&mut self, path: &Path) -> bool {
        let rf = self.recs.iter().find(|r| r.path == path).cloned();
        let ok = media::trash(path);
        if ok {
            if let Some(t) = rf.and_then(|r| r.transcript()) {
                media::trash(&t);
            }
            self.store.tags.remove(&path.to_string_lossy().to_string());
        }
        self.scan_recordings();
        ok
    }

    // ---------------- presets ----------------

    /// Built-in presets plus the person's, for a kind of block.
    pub fn presets_for(&self, kind: &str) -> Vec<(Preset, bool)> {
        crate::presets::builtin_presets()
            .into_iter()
            .filter(|p| p.kind == kind)
            .map(|p| (p, true))
            .chain(self.store.presets.iter().filter(|p| p.kind == kind).cloned().map(|p| (p, false)))
            .collect()
    }

    // ---------------- every frame ----------------

    /// The live work: meters, engines kept in step with edits, timers, events, auto-run, saving. Call often (the
    /// window does it 30 times a second).
    pub fn tick(&mut self) {
        // push edits to running sessions
        for r in &self.running {
            if let Some(s) = self.sessions.iter().find(|s| s.uid == r.uid) {
                *r.engine.shared.blocks.lock().unwrap() = s.blocks.clone();
                *r.engine.shared.session_name.lock().unwrap() = s.name.clone();
            }
        }
        // meters (in ticks of 1/60 s)
        let ticks = (self.last_meter_tick.elapsed().as_secs_f32() * 60.0).round() as u32;
        if ticks >= 1 {
            self.last_meter_tick = Instant::now();
            for r in &self.running {
                let m = r.engine.shared.meters.lock().unwrap().clone();
                for (id, v) in m {
                    self.disp.entry((r.uid, id)).or_default().update(&v, ticks.min(10));
                }
            }
        }
        self.run_timers();
        // sessions set to start with the app start now
        if !self.launched {
            self.launched = true;
            for i in 0..self.sessions.len() {
                if self.sessions[i].auto_run == 1 && !self.is_running(self.sessions[i].uid) {
                    self.toggle_run(i);
                }
            }
        }
        // recordings that just ended
        let mut ended: Vec<(u64, Vec<engine::FileEnd>)> = vec![];
        for r in &self.running {
            let evs: Vec<engine::FileEnd> = std::mem::take(&mut *r.engine.shared.events.lock().unwrap());
            if !evs.is_empty() {
                ended.push((r.uid, evs));
            }
        }
        if !ended.is_empty() {
            self.rec_scanned = None;
        }
        for (uid, evs) in ended {
            self.on_files_ended(uid, evs);
        }
        self.fire_events();
        if self.persist {
            self.track_undo();
            // sound cards plugged in or out show up in the device lists
            if self.last_dev.elapsed().as_secs() >= 5 {
                self.last_dev = Instant::now();
                self.devices = audio::list_devices();
            }
            self.save_if_needed();
        }
    }

    fn save_if_needed(&mut self) {
        if self.last_save.elapsed().as_secs_f32() < 1.0 {
            return;
        }
        self.last_save = Instant::now();
        if let Ok(t) = serde_json::to_string(&self.sessions) {
            if t != self.saved_sessions {
                save_sessions(&self.sessions);
                self.saved_sessions = t;
            }
        }
        if let Ok(t) = serde_json::to_string(&self.store) {
            if t != self.saved_store {
                if let Some(d) = store_path().parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(store_path(), &t);
                self.saved_store = t;
            }
        }
    }

    /// Saves now (before quitting).
    pub fn save_now(&mut self) {
        if self.persist {
            self.last_save = Instant::now() - std::time::Duration::from_secs(5);
            self.save_if_needed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::Script;
    use std::time::Duration;

    fn app() -> Core {
        Core::ephemeral(vec![])
    }

    fn chain(app: &mut Core) -> (u64, u64, u64) {
        let mut s = Session::new("Chain");
        let a = s.add_block("test_tone", 2.0, 2.0);
        let b = s.add_block("volume", 8.0, 2.0);
        let c = s.add_block("recorder", 14.0, 2.0);
        s.connect(a, b);
        s.connect(b, c);
        let dir = engine::test_home().join("chain").to_string_lossy().to_string();
        s.blocks.iter_mut().find(|x| x.id == c).unwrap().p.set_text("folder", &dir);
        app.sessions.push(s);
        (a, b, c)
    }

    fn ev(name: &str, uid: Option<u64>) -> ScriptEvent {
        ScriptEvent { name: name.into(), session: uid, file: None }
    }

    #[test]
    fn automatic_mode_inserts_new_blocks_after_the_selected_one() {
        let mut app = app();
        let (a, b, _c) = chain(&mut app);
        app.sel = Some(a);
        app.add_from_library("balance", None);
        let s = &app.sessions[0];
        let new = app.sel.unwrap();
        assert_eq!(s.block(a).unwrap().outs, vec![new]);
        assert_eq!(s.block(new).unwrap().outs, vec![b]);
        assert!(s.block(new).unwrap().x > s.block(a).unwrap().x);
        app.sel = Some(a);
        app.add_from_library("system_sound", None);
        let src = app.sel.unwrap();
        assert!(app.sessions[0].blocks.iter().all(|x| !x.outs.contains(&src)));
    }

    #[test]
    fn a_block_dropped_on_a_wire_goes_in_the_middle() {
        let mut app = app();
        let (a, b, _c) = chain(&mut app);
        let id = app.insert_on_wire("graphic_eq", a, b, (5.0, 2.0)).unwrap();
        let s = &app.sessions[0];
        assert_eq!(s.block(a).unwrap().outs, vec![id]);
        assert_eq!(s.block(id).unwrap().outs, vec![b]);
        assert_eq!((s.block(id).unwrap().x, s.block(id).unwrap().y), (5.0, 2.0));
    }

    #[test]
    fn manual_mode_leaves_wiring_alone() {
        let mut app = app();
        let (a, b, _) = chain(&mut app);
        app.sessions[0].manual = true;
        app.sel = Some(a);
        app.add_from_library("balance", None);
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

    #[test]
    fn new_sessions_from_templates_get_free_names() {
        let mut app = app();
        let i = app.new_from_template(0);
        let j = app.new_from_template(0);
        assert_ne!(app.sessions[i].name, app.sessions[j].name);
        assert_eq!(app.cur, j);
    }

    #[test]
    fn scripts_start_and_stop_sessions_and_switch_blocks() {
        let mut app = app();
        let (_, _, c) = chain(&mut app);
        app.store.scripts = vec![Script {
            name: "T".into(),
            text: "kalango.log('hi'); kalango.start('chain'); ctx.session.blocks.forEach(b => { if (b.title == 'Recorder') b.bypassed = true; });".into(),
        }];
        let uid = app.sessions[0].uid;
        app.run_script("T", &ev("manual", Some(uid)));
        assert!(app.is_running(uid), "the script started the session");
        assert!(app.log.iter().any(|l| l.ends_with("hi")));
        assert!(!app.sessions[0].block(c).unwrap().enabled, "the recorder block was switched off");
        app.store.scripts.push(Script { name: "S".into(), text: "kalango.stopAll(); kalango.start('No Such');".into() });
        app.run_script("S", &ev("manual", None));
        assert!(!app.is_running(uid));
        assert!(app.log.iter().any(|l| l.contains("no session called “No Such”")));
        app.store.scripts.push(Script { name: "Bad".into(), text: "this is not (javascript".into() });
        app.run_script("Bad", &ev("manual", None));
        assert!(app.log.iter().any(|l| l.contains("stopped with an error")));
        app.run_script("Nope", &ev("manual", None));
        assert!(app.log.iter().any(|l| l.contains("no script called “Nope”")));
    }

    #[test]
    fn the_quit_script_asks_the_program_to_close() {
        let mut app = app();
        chain(&mut app);
        let quit = script::BUILTIN.iter().map(|(n, _)| *n).last().unwrap();
        app.run_script(quit, &ev("manual", None));
        assert!(app.quit_now);
    }

    #[test]
    fn automations_run_scripts_on_the_five_events() {
        let mut app = app();
        chain(&mut app);
        app.store.scripts = vec![Script { name: "Hello".into(), text: "kalango.log('hello ' + ctx.kind);".into() }];
        app.sessions[0].automations =
            EVENTS.iter().map(|(e, _)| Automation { event: e.to_string(), trigger: 0, minutes: 0, script: "Hello".into(), on: true }).collect();
        app.sessions[0].automations.push(Automation { event: "session_starting".into(), trigger: 0, minutes: 0, script: "Hello".into(), on: false });
        let uid = app.sessions[0].uid;
        app.toggle_run(0); // queues session_starting
        app.tick();
        let hellos = |a: &Core, e: &str| a.log.iter().filter(|l| l.ends_with(&format!("hello {e}"))).count();
        assert_eq!(hellos(&app, "session_starting"), 1, "the switched-off automation must not run");
        app.toggle_run(0);
        app.tick();
        assert_eq!(hellos(&app, "session_stopped"), 1);
        let before_files = hellos(&app, "recording_finished"); // stopping the session also finished a real file
        app.queue_event("timer_starting", Some(uid), None);
        app.queue_event("timer_stopped", Some(uid), None);
        app.on_files_ended(uid, vec![engine::FileEnd { block: 1, path: "/tmp/x/a.wav".into(), seconds: 2.0, size: 10 }]);
        app.tick();
        assert_eq!(hellos(&app, "timer_starting"), 1);
        assert_eq!(hellos(&app, "timer_stopped"), 1);
        assert_eq!(hellos(&app, "recording_finished"), before_files + 1);
        // events of another session do not trigger this session's automations
        app.queue_event("session_starting", Some(uid + 1), None);
        app.tick();
        assert_eq!(hellos(&app, "session_starting"), 1);
    }

    #[test]
    fn a_finished_recording_reaches_its_script_as_event_file() {
        let mut app = app();
        chain(&mut app);
        app.store.scripts = vec![Script { name: "F".into(), text: "kalango.log(ctx.recording.name, ctx.recording.seconds, ctx.session.name);".into() }];
        app.sessions[0].automations = vec![Automation { event: "recording_finished".into(), trigger: 0, minutes: 0, script: "F".into(), on: true }];
        let uid = app.sessions[0].uid;
        app.on_files_ended(uid, vec![engine::FileEnd { block: 3, path: "/tmp/x/take 1.wav".into(), seconds: 4.5, size: 99 }]);
        app.tick();
        assert!(app.log.iter().any(|l| l.ends_with("take 1.wav 4.5 Chain")), "{:?}", app.log);
    }

    #[test]
    fn timer_windows_open_on_their_date_or_weekdays() {
        let (wd, min, today) = (3u32, 600u32, 20261119u32); // a Wednesday, 10:00
        let base = Timer { days: 0, start: 540, minutes: 120, on: true, mode: 1, date: 0, quit_sources: false };
        assert!(!timer_open(&base, wd, min, today));
        assert!(timer_open(&Timer { days: 1 << 3, ..base.clone() }, wd, min, today));
        assert!(!timer_open(&Timer { days: 1 << 3, start: 700, ..base.clone() }, wd, min, today));
        assert!(!timer_open(&Timer { days: 1 << 3, on: false, ..base.clone() }, wd, min, today));
        assert!(timer_open(&Timer { mode: 0, date: today, ..base.clone() }, wd, min, today));
        assert!(!timer_open(&Timer { mode: 0, date: today + 1, days: 0b1111111, ..base.clone() }, wd, min, today));
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
        assert!(app.queue.iter().any(|e| e.name == "timer_starting"));
        app.sessions[0].timers[0].on = false;
        app.last_timer_check = Instant::now() - Duration::from_secs(5);
        app.run_timers();
        assert!(!app.is_running(app.sessions[0].uid), "a session the timer started stops with it");
        assert!(app.queue.iter().any(|e| e.name == "timer_stopped"));
        app.toggle_run(0);
        app.last_timer_check = Instant::now() - Duration::from_secs(5);
        app.run_timers();
        assert!(app.is_running(app.sessions[0].uid), "a session started by hand is never stopped by a timer");
        app.toggle_run(0);
    }

    #[test]
    fn sessions_marked_auto_run_start_when_the_program_opens() {
        let mut app = app();
        chain(&mut app);
        app.sessions[0].auto_run = 1;
        app.launched = false;
        app.tick();
        assert!(app.is_running(app.sessions[0].uid));
        app.toggle_run(0);
    }

    #[test]
    fn quitting_asks_first_only_while_recording() {
        let mut app = app();
        chain(&mut app);
        app.request_quit();
        assert!(app.quit_now, "nothing running: quit at once");
        app.quit_now = false;
        let rec = app.sessions[0].blocks.iter().find(|b| b.kind == "recorder").unwrap().id;
        app.sessions[0].blocks.iter_mut().find(|b| b.id == rec).unwrap().enabled = false;
        app.toggle_run(0);
        app.request_quit();
        assert!(app.quit_now && app.quit_state == 0, "playing without recording: quit at once");
        app.quit_now = false;
        app.sessions[0].blocks.iter_mut().find(|b| b.id == rec).unwrap().enabled = true;
        app.toggle_run(0);
        app.request_quit();
        assert_eq!(app.quit_state, 1);
        assert!(!app.quit_now);
        app.confirm_quit(false);
        assert!(!app.quit_now && !app.running.is_empty(), "cancel keeps recording");
        app.request_quit();
        app.confirm_quit(true);
        assert!(app.quit_now && app.running.is_empty(), "sessions are stopped, then the program closes");
    }

    #[test]
    fn sessions_can_be_exported_and_imported_again() {
        let mut app = app();
        chain(&mut app);
        let path = engine::test_home().join("roundtrip.json").to_string_lossy().to_string();
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
        assert_eq!(app.sessions[1].name, "Chain (copy)");
        assert_eq!(app.cur, 1);
        app.delete_session();
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.cur, 0);
    }

    #[test]
    fn running_sessions_light_their_meters_and_recordings_show_up() {
        let mut app = app();
        let (_, b, c) = chain(&mut app);
        let uid = app.sessions[0].uid;
        app.store.prefs.default_folder = engine::test_home().join("nothing-here").to_string_lossy().to_string();
        app.toggle_run(0);
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(30));
            app.tick();
        }
        assert!(app.disp.get(&(uid, b)).map(|d| d.peak_max() > 0.1).unwrap_or(false), "the volume block's meter moves");
        assert!(app.is_recording(uid));
        assert!(app.rec_status(uid, c).is_some());
        app.toggle_run(0);
        assert!(app.disp.keys().all(|(s, _)| *s != uid), "a stopped session falls quiet");
        app.scan_recordings();
        assert!(app.recs.iter().any(|r| r.name.ends_with(".wav")), "{:?}", app.recs);
        let p = app.recs[0].path.clone();
        app.set_tags(&p, vec!["reunião".into()]);
        assert_eq!(app.tags_of(&p), vec!["reunião".to_string()]);
    }

    #[test]
    fn edits_can_be_undone_and_redone() {
        let mut app = app();
        let (_, b, _) = chain(&mut app);
        app.track_undo(); // baseline
        let gain = |a: &Core| a.sessions[0].block(b).unwrap().p.num("gain", 9.0);
        let set = |a: &mut Core, v: f64| {
            a.sessions[0].blocks.iter_mut().find(|x| x.id == b).unwrap().p.set("gain", v);
        };
        let settle = |a: &mut Core| {
            a.track_undo();
            a.last_change = Instant::now() - Duration::from_secs(2);
            a.track_undo();
        };
        set(&mut app, 0.5);
        settle(&mut app);
        set(&mut app, 0.25);
        settle(&mut app);
        assert_eq!(gain(&app), 0.25);
        app.undo();
        assert_eq!(gain(&app), 0.5);
        app.undo();
        assert_eq!(gain(&app), 1.0);
        app.undo();
        assert_eq!(gain(&app), 1.0);
        app.redo();
        assert_eq!(gain(&app), 0.5);
        app.redo();
        assert_eq!(gain(&app), 0.25);
        app.undo();
        set(&mut app, 0.9);
        settle(&mut app);
        assert!(app.redo.is_empty());
        set(&mut app, 0.1);
        app.undo();
        assert_eq!(gain(&app), 0.9);
        assert_eq!(app.sessions.len(), 1);
    }

    #[test]
    fn old_saved_sessions_still_open_and_damaged_files_do_not_crash() {
        let old = r#"[{"name":"Old","blocks":[{"id":1,"kind":"test_tone","x":2.0,"y":2.0,"enabled":true,"outs":[],"p":{"n":{"freq":440.0},"s":{}}}],"next_id":2,"timers":[{"days":62,"start":540,"minutes":60,"on":true}],"automations":[{"trigger":1,"minutes":5,"script":"X","on":true}]}]"#;
        let mut v: Vec<Session> = serde_json::from_str(old).unwrap();
        assert!(!v[0].manual && v[0].notes.is_empty() && v[0].auto_run == 0);
        assert_eq!(v[0].timers[0].mode, 1, "old timers repeat on weekdays");
        for a in v[0].automations.iter_mut() {
            a.migrate();
        }
        assert_eq!(v[0].automations[0].event, "session_stopped");
        assert!(serde_json::from_str::<Vec<Session>>("{ broken").is_err());
    }
}
