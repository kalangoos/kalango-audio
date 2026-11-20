//! Session data: blocks on a grid, the wires between them, templates and saving.
use crate::i18n::n_;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize, PartialEq, Default, Debug)]
pub struct Params {
    #[serde(default)]
    pub n: BTreeMap<String, f64>,
    #[serde(default)]
    pub s: BTreeMap<String, String>,
}

impl Params {
    pub fn num(&self, k: &str, d: f64) -> f64 { self.n.get(k).copied().unwrap_or(d) }
    pub fn text(&self, k: &str, d: &str) -> String { self.s.get(k).cloned().unwrap_or_else(|| d.to_string()) }
    pub fn set(&mut self, k: &str, v: f64) { self.n.insert(k.to_string(), v); }
    pub fn set_text(&mut self, k: &str, v: &str) { self.s.insert(k.to_string(), v.to_string()); }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Block {
    pub id: u64,
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub enabled: bool,
    pub outs: Vec<u64>,
    pub p: Params,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Timer {
    /// bit 0 = Sunday ... bit 6 = Saturday (used by "Repeat Every")
    pub days: u8,
    /// "From" time, minutes after midnight
    pub start: u16,
    /// length in minutes ("To" time = From + length)
    pub minutes: u16,
    pub on: bool,
    /// 0 = On Date, 1 = Repeat Every
    #[serde(default = "one")]
    pub mode: u8,
    /// the day for "On Date", as yyyymmdd
    #[serde(default)]
    pub date: u32,
    /// "Quit sources when done"
    #[serde(default)]
    pub quit_sources: bool,
}

fn one() -> u8 { 1 }

impl Timer {
    pub fn weekdays() -> Timer { Timer { days: 0b0111110, start: 9 * 60, minutes: 60, on: true, mode: 1, date: 0, quit_sources: false } }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Automation {
    /// recording_finished, session_starting, session_stopped, timer_starting or timer_stopped
    #[serde(default)]
    pub event: String,
    /// older saved files: 0 = session starts, 1 = session stops
    #[serde(default)]
    pub trigger: u8,
    #[serde(default)]
    pub minutes: u32,
    /// name of the script to run (built-in or the user's); it sees the event id as `ctx.kind`
    pub script: String,
    pub on: bool,
}

/// Events an automation can react to: (id passed to scripts as `ctx.kind`, label shown in the UI).
pub const EVENTS: [(&str, &str); 5] = [
    ("recording_finished", n_("Recording finished")),
    ("session_starting", n_("Session started")),
    ("session_stopped", n_("Session stopped")),
    ("timer_starting", n_("Schedule started")),
    ("timer_stopped", n_("Schedule ended")),
];

impl Automation {
    pub fn new() -> Automation { Automation { event: "session_starting".into(), trigger: 0, minutes: 0, script: String::new(), on: true } }
    /// Files saved by an earlier version used a number instead of an event name.
    pub fn migrate(&mut self) {
        if self.event.is_empty() { self.event = if self.trigger == 1 { "session_stopped".into() } else { "session_starting".into() }; }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Session {
    #[serde(default)]
    pub notes: String,
    /// 0 = Off, 1 = On Launch
    #[serde(default)]
    pub auto_run: u8,
    /// session sample rate in Hz (0 = 48000)
    #[serde(default)]
    pub sample_rate: u32,
    #[serde(default)]
    pub automations: Vec<Automation>,
    /// manual connections mode: blocks are wired by hand instead of automatically
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub timers: Vec<Timer>,
    #[serde(default)]
    pub uid: u64,
    pub name: String,
    pub blocks: Vec<Block>,
    pub next_id: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Cat { Source, Output, Effect, Meter, Advanced }

pub struct KindInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub desc: &'static str,
    pub cat: Cat,
    /// false for blocks that only pass the audio through for now
    pub working: bool,
}

pub const KINDS: &[KindInfo] = &[
    KindInfo { id: "app_source", title: n_("Application"), desc: n_("Capture the sound of an open program"), cat: Cat::Source, working: true },
    KindInfo { id: "device_input", title: n_("Microphone or input"), desc: n_("Capture a microphone or audio interface"), cat: Cat::Source, working: true },
    KindInfo { id: "system_sound", title: n_("System sound"), desc: n_("Capture everything the computer is playing"), cat: Cat::Source, working: true },
    KindInfo { id: "test_tone", title: n_("Test tone"), desc: n_("Generate a steady tone to try the session"), cat: Cat::Source, working: true },
    KindInfo { id: "device_output", title: n_("Audio output"), desc: n_("Play the sound on speakers or headphones"), cat: Cat::Output, working: true },
    KindInfo { id: "recorder", title: n_("Recorder"), desc: n_("Save the sound to a file"), cat: Cat::Output, working: true },
    KindInfo { id: "rtmp_stream", title: n_("RTMP stream"), desc: n_("Send the sound live to an RTMP server"), cat: Cat::Output, working: true },
    KindInfo { id: "radio_stream", title: n_("Online radio"), desc: n_("Send the sound to an Icecast-compatible server"), cat: Cat::Output, working: true },
    KindInfo { id: "graphic_eq", title: n_("Graphic equalizer"), desc: n_("Ten frequency bands, from 31 Hz to 16 kHz"), cat: Cat::Effect, working: true },
    KindInfo { id: "balance", title: n_("Balance"), desc: n_("Even out the left and right sides"), cat: Cat::Effect, working: true },
    KindInfo { id: "tone_control", title: n_("Bass and treble"), desc: n_("Two simple tone controls"), cat: Cat::Effect, working: true },
    KindInfo { id: "channels", title: n_("Channels"), desc: n_("Swap, join or isolate the channels"), cat: Cat::Effect, working: true },
    KindInfo { id: "low_pass", title: n_("Low-pass"), desc: n_("Cut the high frequencies"), cat: Cat::Effect, working: true },
    KindInfo { id: "loudness", title: n_("Fuller sound"), desc: n_("Gentle compression that makes the sound fuller"), cat: Cat::Effect, working: true },
    KindInfo { id: "stereo_widen", title: n_("Stereo widener"), desc: n_("Spread a mono signal across both sides"), cat: Cat::Effect, working: true },
    KindInfo { id: "pan", title: n_("Pan"), desc: n_("Place the sound between left and right"), cat: Cat::Effect, working: true },
    KindInfo { id: "compressor", title: n_("Compressor"), desc: n_("Hold the peaks and lift the quiet parts"), cat: Cat::Effect, working: true },
    KindInfo { id: "volume", title: n_("Volume"), desc: n_("Make the sound louder or quieter"), cat: Cat::Effect, working: true },
    KindInfo { id: "level_meter", title: n_("Level meter"), desc: n_("Show peak and average as bars"), cat: Cat::Meter, working: true },
    KindInfo { id: "needle_meter", title: n_("Needle meter"), desc: n_("Show the level on a needle"), cat: Cat::Meter, working: true },
    KindInfo { id: "click_removal", title: n_("Click removal"), desc: n_("Take short clicks out of the sound"), cat: Cat::Advanced, working: true },
    KindInfo { id: "hum_removal", title: n_("Hum removal"), desc: n_("Take out mains hum (50/60 Hz)"), cat: Cat::Advanced, working: true },
    KindInfo { id: "source_switch", title: n_("Source switch"), desc: n_("Choose which of two sources goes on"), cat: Cat::Advanced, working: true },
    KindInfo { id: "transcribe", title: n_("Transcribe"), desc: n_("Turn speech into text while recording"), cat: Cat::Advanced, working: true },
    KindInfo { id: "ladspa_plugin", title: n_("LADSPA plug-in"), desc: n_("Use a LADSPA effect installed on the system"), cat: Cat::Advanced, working: true },
    KindInfo { id: "voice_denoise", title: n_("Clean voice"), desc: n_("Remove background noise from speech (RNNoise)"), cat: Cat::Advanced, working: true },
    KindInfo { id: "convolution", title: n_("Convolution"), desc: n_("Apply an impulse response (FIR) from a file"), cat: Cat::Advanced, working: true },
    KindInfo { id: "noise_reduction", title: n_("Noise reduction"), desc: n_("Learn the background noise and lower it"), cat: Cat::Advanced, working: true },
    KindInfo { id: "ducking", title: n_("Duck under voice"), desc: n_("Lower the other sources when someone speaks"), cat: Cat::Advanced, working: true },
    KindInfo { id: "mixer", title: n_("Mixer"), desc: n_("Mix up to five sources"), cat: Cat::Advanced, working: true },
    KindInfo { id: "parametric_eq", title: n_("Parametric EQ"), desc: n_("Up to 16 bands with frequency, gain and Q"), cat: Cat::Advanced, working: true },
    KindInfo { id: "delay", title: n_("Delay"), desc: n_("Delay the sound by milliseconds to sync it"), cat: Cat::Advanced, working: true },
    KindInfo { id: "time_shift", title: n_("Time shift"), desc: n_("Pause and rewind the live sound"), cat: Cat::Advanced, working: true },
    KindInfo { id: "high_pass", title: n_("High-pass"), desc: n_("Cut the low frequencies"), cat: Cat::Effect, working: true },
    KindInfo { id: "band_pass", title: n_("Band-pass"), desc: n_("Let only one range of frequencies through"), cat: Cat::Effect, working: true },
    KindInfo { id: "high_shelf", title: n_("Treble boost"), desc: n_("Raise or cut above a frequency"), cat: Cat::Effect, working: true },
    KindInfo { id: "low_shelf", title: n_("Bass boost"), desc: n_("Raise or cut below a frequency"), cat: Cat::Effect, working: true },
];

pub fn kind_info(id: &str) -> Option<&'static KindInfo> { KINDS.iter().find(|k| k.id == id) }
pub fn is_source(kind: &str) -> bool { matches!(kind, "app_source" | "device_input" | "system_sound" | "test_tone") }

pub fn default_params(kind: &str) -> Params {
    let mut p = Params::default();
    match kind {
        "volume" => { p.set("gain", 1.0); p.set("boost", 1.0); }
        "device_output" => { p.set("gain", 1.0); p.set("muted", 0.0); p.set_text("device", "default"); }
        "device_input" | "system_sound" | "app_source" => {
            p.set_text("device", "default");
            if kind == "app_source" { p.set_text("app", "Web Browser"); }
        }
        "test_tone" => { p.set("freq", 440.0); p.set("level", 0.25); }
        "graphic_eq" => for i in 0..10 { p.set(&format!("band{i}"), 0.5) },
        "balance" => p.set("balance", 0.0),
        "pan" => p.set("pan", 0.0),
        "tone_control" => { p.set("bass", 0.5); p.set("treble", 0.5); }
        "low_pass" => p.set("freq", 10000.0),
        "compressor" => { p.set("amount", 1.0); p.set("speed", 0.0); p.set("makeup", 1.0); }
        "loudness" => { p.set("amount", 0.0); p.set("speed", 0.0); p.set("makeup", 1.0); }
        "channels" => p.set("mode", 4.0),
        "hum_removal" => { p.set("base", 50.0); p.set("amount", 3.0); }
        "click_removal" => p.set("sens", 3.0),
        "mixer" => for i in 0..5 { p.set(&format!("g{i}"), 1.0); p.set(&format!("m{i}"), 0.0); p.set(&format!("s{i}"), 0.0); p.set(&format!("b{i}"), 0.0); p.set(&format!("p{i}"), 0.0); p.set(&format!("fo{i}"), 0.0); },
        "voice_denoise" => p.set("mono", 0.0),
        "stereo_widen" => p.set("crossover", 500.0),
        "ladspa_plugin" => p.set_text("plugin", ""),
        "convolution" => { p.set_text("ir", ""); p.set("pregain", 0.0); }
        "ducking" => { p.set("threshold", -30.0); p.set("ducked", 0.4); p.set("overlay", 0.0); }
        "noise_reduction" => { p.set("reduction", 6.0); p.set("learn_seq", 0.0); }
        "delay" => p.set("delay", 0.0),
        "time_shift" => { p.set("paused", 0.0); p.set("jump_seq", 0.0); p.set("live_seq", 0.0); p.set("jump_s", 0.0); p.set("jump_live", 0.0); }
        "source_switch" => p.set("switch", 0.0),
        "high_pass" => { p.set("freq", 80.0); }
        "band_pass" => { p.set("freq", 1000.0); p.set("q", 1.0); }
        "high_shelf" => { p.set("freq", 6000.0); p.set("gain", 0.0); }
        "low_shelf" => { p.set("freq", 150.0); p.set("gain", 0.0); }
        "parametric_eq" => {
            p.set("bands", 3.0);
            for (i, (t, f, q)) in [(1.0, 100.0, 0.7), (0.0, 1000.0, 1.0), (2.0, 8000.0, 0.7)].into_iter().enumerate() {
                p.set(&format!("b{i}_type"), t); p.set(&format!("b{i}_freq"), f); p.set(&format!("b{i}_gain"), 0.0); p.set(&format!("b{i}_q"), q); p.set(&format!("b{i}_on"), 1.0);
            }
        }
        "radio_stream" => {
            p.set_text("server", "Icecast 2"); p.set_text("host", ""); p.set("port", 8000.0); p.set_text("user", "source"); p.set_text("password", "");
            p.set_text("mount", "/stream"); p.set_text("name", ""); p.set_text("genre", ""); p.set_text("url", ""); p.set("public", 0.0);
            p.set_text("format", "MP3"); p.set("bitrate", 128.0); p.set_text("stream_id", "");
        }
        "rtmp_stream" => { p.set_text("url", ""); p.set_text("key", ""); p.set("quality", 1.0); }
        "transcribe" => {
            p.set_text("command", "whisper-cli"); p.set_text("model", ""); p.set_text("language", "auto");
            p.set_text("folder", &crate::engine::default_transcripts_dir()); p.set_text("file_name", crate::engine::DEFAULT_TRANSCRIPT_NAME);
        }
        "recorder" => {
            p.set_text("file_name", crate::engine::DEFAULT_FILE_NAME);
            p.set_text("folder", &crate::engine::default_recordings_dir());
            p.set_text("format", "WAV");
            p.set("bitrate", 256.0);
            p.set("depth", 16.0); p.set("dither", 0.0);
            p.set("split_on", 0.0); p.set("split_size", 5.0); p.set_text("split_unit", "space*1");
            p.set("stop_on", 0.0); p.set("stop_size", 1.0); p.set_text("stop_unit", "time*3600");
            p.set("silence_on", 0.0); p.set("silence_action", 1.0); p.set("silence_time", crate::engine::SILENCE_SECONDS); p.set("silence_db", crate::engine::SILENCE_DB);
            p.set("pause", 0.0); p.set("split_req", 0.0);
            for k in ["title", "artist", "album", "genre", "comment", "year", "track", "trackcount", "disc", "disccount", "art"] { p.set_text(&format!("tag_{k}"), ""); }
            p.set("tag_compilation", 0.0);
            p.set("q_rate", 0.0); p.set("q_channels", 0.0); p.set("q_vbr", 0.0); p.set("q_mp4", 0.0);
        }
        _ => {}
    }
    p
}

impl Session {
    pub fn new(name: &str) -> Self { Session { notes: String::new(), auto_run: 0, sample_rate: 0, automations: vec![], manual: false, timers: vec![], uid: new_uid(), name: name.to_string(), blocks: vec![], next_id: 1 } }

    pub fn add_block(&mut self, kind: &str, x: f32, y: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.blocks.push(Block { id, kind: kind.to_string(), x, y, enabled: true, outs: vec![], p: default_params(kind) });
        id
    }

    pub fn remove_block(&mut self, id: u64) {
        self.blocks.retain(|b| b.id != id);
        for b in self.blocks.iter_mut() { b.outs.retain(|o| *o != id); }
    }

    pub fn connect(&mut self, from: u64, to: u64) {
        if from == to { return; }
        // sources have no input, so nothing can feed them
        if let Some(t) = self.blocks.iter().find(|b| b.id == to) { if is_source(&t.kind) { return; } }
        if let Some(b) = self.blocks.iter_mut().find(|b| b.id == from) {
            if !b.outs.contains(&to) { b.outs.push(to); }
        }
    }

    pub fn disconnect(&mut self, from: u64, to: u64) {
        if let Some(b) = self.blocks.iter_mut().find(|b| b.id == from) { b.outs.retain(|o| *o != to); }
    }

    pub fn block(&self, id: u64) -> Option<&Block> { self.blocks.iter().find(|b| b.id == id) }
}

// ---- templates (our own starting points, in templates.json) ----


#[derive(Deserialize)]
pub struct TplBlock {
    pub kind: String,
    /// parameters that differ from `default_params` (numbers or text)
    #[serde(default)]
    pub p: BTreeMap<String, serde_json::Value>,
    /// indexes of the blocks this one feeds
    #[serde(default)]
    pub out: Vec<usize>,
    /// grid position, worked out by `templates()` (left to right, following the wires)
    #[serde(skip)]
    pub x: f32,
    #[serde(skip)]
    pub y: f32,
}

#[derive(Deserialize)]
pub struct Template {
    pub name: String,
    pub subtitle: String,
    pub desc: String,
    /// gallery group: Básico, Gravar, Voz, Ouvir, Transmitir
    #[serde(default)]
    pub category: String,
    /// short generic icon name for the gallery
    #[serde(default)]
    pub icon: String,
    /// the session starts in manual connections mode
    #[serde(default)]
    pub manual: bool,
    pub blocks: Vec<TplBlock>,
}

pub fn templates() -> Vec<Template> {
    let mut v: Vec<Template> = serde_json::from_str(include_str!("templates.json")).unwrap_or_default();
    for t in v.iter_mut() { t.layout(); }
    v
}

impl Template {
    /// Places blocks in columns by their distance from the sources (6 grid units apart) and
    /// stacks each column's blocks 5 units apart, centred on the tallest column.
    fn layout(&mut self) {
        let n = self.blocks.len();
        let mut col = vec![0usize; n];
        // longest path from a block with no inputs; templates have no loops, n passes are enough
        for _ in 0..n {
            for i in 0..n {
                for &o in &self.blocks[i].out { if o < n && col[o] < col[i] + 1 { col[o] = col[i] + 1; } }
            }
        }
        let cols = col.iter().copied().max().map(|m| m + 1).unwrap_or(0);
        let height = (0..cols).map(|c| col.iter().filter(|&&k| k == c).count()).max().unwrap_or(0);
        let mut row = vec![0usize; cols];
        for i in 0..n {
            let c = col[i];
            let in_col = col.iter().filter(|&&k| k == c).count();
            let b = &mut self.blocks[i];
            b.x = 2.0 + 6.0 * c as f32;
            b.y = 2.0 + 5.0 * row[c] as f32 + 2.5 * (height - in_col) as f32;
            row[c] += 1;
        }
    }

    pub fn instantiate(&self) -> Session {
        let mut s = Session::new(crate::i18n::tr(&self.name));
        s.manual = self.manual;
        let mut ids = vec![];
        for tb in &self.blocks {
            let id = s.add_block(&tb.kind, tb.x, tb.y);
            ids.push(id);
            let b = s.blocks.last_mut().unwrap();
            match tb.kind.as_str() {
                // new recordings and transcripts follow the user's XDG folders, unless the template says otherwise
                "recorder" => b.p.set_text("folder", &crate::engine::default_recordings_dir()),
                "transcribe" => b.p.set_text("folder", &crate::engine::default_transcripts_dir()),
                _ => {}
            }
            for (k, v) in &tb.p {
                match v {
                    serde_json::Value::Number(n) => b.p.set(k, n.as_f64().unwrap_or(0.0)),
                    serde_json::Value::String(t) => b.p.set_text(k, t),
                    serde_json::Value::Bool(on) => b.p.set(k, if *on { 1.0 } else { 0.0 }),
                    _ => {}
                }
            }
        }
        for (i, tb) in self.blocks.iter().enumerate() {
            for o in &tb.out {
                if let Some(to) = ids.get(*o) { s.connect(ids[i], *to); }
            }
        }
        s
    }
}

// ---- saving ----

pub fn config_path() -> std::path::PathBuf {
    // tests never write into the real home folder
    #[cfg(test)]
    let base = crate::engine::test_home().join("config");
    #[cfg(not(test))]
    let base = std::env::var("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
    });
    base.join("kalango").join("audio").join("sessions.json")
}

pub fn new_uid() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    t.wrapping_add(N.fetch_add(1, Ordering::Relaxed))
}

pub fn load_sessions() -> Vec<Session> {
    if let Ok(t) = std::fs::read_to_string(config_path()) {
        if let Ok(mut v) = serde_json::from_str::<Vec<Session>>(&t) {
            for s in v.iter_mut() { if s.uid == 0 { s.uid = new_uid(); } for a in s.automations.iter_mut() { a.migrate(); } }
            return v;
        }
    }
    let tpls = templates();
    ["Record the computer", "Clean narration", "Online meeting"]
        .iter()
        .filter_map(|n| tpls.iter().find(|t| &t.name == n))
        .map(|t| t.instantiate())
        .collect()
}

pub fn save_sessions(v: &[Session]) {
    let p = config_path();
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    if let Ok(t) = serde_json::to_string_pretty(v) { let _ = std::fs::write(p, t); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine;

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

    #[test]
    fn no_template_silences_its_compressors() {
        // make-up is a plain gain: 0 would mute everything after the compressor
        for tpl in templates() {
            for b in tpl.instantiate().blocks.iter().filter(|b| b.kind == "compressor" || b.kind == "loudness") {
                assert!(b.p.num("makeup", 1.0) >= 1.0, "{}: make-up {}", tpl.name, b.p.num("makeup", 1.0));
            }
        }
    }
}
