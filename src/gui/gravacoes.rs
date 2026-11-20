//! Recordings (design "Kalango Audio · Gravações (biblioteca)"): the library of recordings grouped by day, with a
//! search that also looks inside the transcripts, filter chips, playback in each row, and the detail panel on the
//! right — big waveform with a playhead to seek, transport, editable tags, the transcript that follows playback,
//! copy / .srt export / summary by the agent, show in Files, open in the editor, move to the trash (with undo).
//! In a medium or narrow window the detail becomes its own page with a back button.

use super::widgets::*;
use super::{icons, Change, Layout, Ui};
use crate::core::{expand_home, fmt_duration, fmt_size, Core, RecFile};
use crate::i18n::{tr, trf};
use crate::media::Player;
use gtk::prelude::*;
use gtk::{gdk, glib, Align};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

// ---------------------------------------------------------------- pure helpers

/// One line of a transcript: "[00:12] Speaker: text" (time and speaker are optional).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub at: Option<f64>,
    pub speaker: String,
    pub text: String,
}

/// "00:12", "1:02:03", "00:00:05.120" → seconds.
pub fn parse_stamp(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut t = 0.0;
    for p in &parts {
        let v: f64 = p.trim().replace(',', ".").parse().ok()?;
        if v < 0.0 {
            return None;
        }
        t = t * 60.0 + v;
    }
    Some(t)
}

/// Reads a transcript: "[mm:ss] Speaker: text" lines ("[00:00:00.000 --> 00:00:05.000] text" subtitle-style lines too); lines
/// without a time become rows of their own.
pub fn parse_transcript(s: &str) -> Vec<Line> {
    let mut out = vec![];
    for raw in s.lines() {
        let l = raw.trim();
        if l.is_empty() {
            continue;
        }
        let mut at = None;
        let mut rest = l;
        if let Some(inner) = l.strip_prefix('[').and_then(|r| r.split_once(']')) {
            let stamp = inner.0.split("-->").next().unwrap_or("");
            if let Some(t) = parse_stamp(stamp) {
                at = Some(t);
                rest = inner.1.trim();
            }
        }
        let mut speaker = String::new();
        let mut text = rest.to_string();
        if at.is_some() {
            if let Some((who, said)) = rest.split_once(": ") {
                if !who.is_empty() && who.chars().count() <= 40 && !who.contains(['.', '?', '!', '[']) {
                    speaker = who.trim().to_string();
                    text = said.trim().to_string();
                }
            }
        }
        out.push(Line { at, speaker, text });
    }
    out
}

/// The line being said at `pos` seconds (the last one that started).
pub fn current_line(lines: &[Line], pos: f64) -> Option<usize> {
    lines.iter().enumerate().rev().find(|(_, l)| l.at.is_some_and(|t| t <= pos + 0.05)).map(|(i, _)| i)
}

fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
}

/// "00:12", "16:44", "1:02:03" (the transcript's times)
pub fn fmt_stamp(t: f64) -> String {
    let s = t.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// SubRip subtitles from the timed lines: each lasts until the next one (the last one 5 s, or to the end).
pub fn to_srt(lines: &[Line], total: Option<f64>) -> String {
    let timed: Vec<&Line> = lines.iter().filter(|l| l.at.is_some()).collect();
    let mut out = String::new();
    for (i, l) in timed.iter().enumerate() {
        let start = l.at.unwrap_or(0.0);
        let mut end = timed.get(i + 1).and_then(|n| n.at).unwrap_or(start + 5.0);
        if let Some(t) = total.filter(|t| *t > start) {
            end = end.min(t);
        }
        if end <= start {
            end = start + 1.0;
        }
        let text = if l.speaker.is_empty() { l.text.clone() } else { format!("{}: {}", l.speaker, l.text) };
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(start), srt_time(end), text));
    }
    out
}

/// "Reunião semanal 2026-11-19 14.02.10" → "Reunião semanal" (the date and time the Recorder adds are shown apart).
pub fn display_name(stem: &str) -> String {
    let s = stem.trim();
    // "2026-11-19 21.51.52 Rádio" (the date first)
    if s.len() > 20 && s.is_char_boundary(20) && s.as_bytes()[19] == b' ' && display_name(&format!("x {}", &s[..19])) == "x" {
        return s[20..].trim().to_string();
    }
    let b = s.as_bytes();
    // " YYYY-MM-DD HH.MM.SS" or " YYYY-MM-DD HH-MM-SS" or " YYYY-MM-DD"
    for tail in [20usize, 11] {
        if b.len() > tail && b[b.len() - tail] == b' ' {
            let t = &s[s.len() - tail + 1..];
            let digits_ok = t.chars().enumerate().all(|(i, c)| match i {
                4 | 7 => c == '-',
                10 => c == ' ' || c == '_',
                13 | 16 => matches!(c, '.' | '-' | ':'),
                _ => c.is_ascii_digit(),
            });
            if digits_ok {
                return s[..s.len() - tail].trim().to_string();
            }
        }
    }
    // " HH.MM.SS" alone (a Recorder named "… %time")
    if b.len() > 9 && b[b.len() - 9] == b' ' {
        let t = &s[s.len() - 8..];
        if t.chars().enumerate().all(|(i, c)| if i == 2 || i == 5 { matches!(c, '.' | '-' | ':') } else { c.is_ascii_digit() }) {
            let rest = s[..s.len() - 9].trim();
            let is_date = rest.len() == 10 && rest.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
            if !is_date {
                return rest.to_string();
            }
        }
    }
    s.to_string()
}

/// How a recording is grouped: by how many days ago it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Group {
    Today,
    Yesterday,
    ThisWeek,
    /// day of month, month, year
    Date(u32, u32, i32),
}

/// Whole days between local midnight today (`midnight`, epoch seconds) and `when`.
pub fn days_ago(when: u64, midnight: u64) -> u64 {
    if when >= midnight {
        0
    } else {
        (midnight - when - 1) / 86_400 + 1
    }
}

pub fn group_of(days: u64, date: (u32, u32, i32)) -> Group {
    match days {
        0 => Group::Today,
        1 => Group::Yesterday,
        2..=6 => Group::ThisWeek,
        _ => Group::Date(date.0, date.1, date.2),
    }
}

/// "48 min 02 s", "1 h 12 min", "6 s"
pub fn long_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        trf("{h} h {m} min", &[("h", (s / 3600).to_string()), ("m", format!("{:02}", (s / 60) % 60))])
    } else if s >= 60 {
        trf("{m} min {s} s", &[("m", (s / 60).to_string()), ("s", format!("{:02}", s % 60))])
    } else {
        trf("{s} s", &[("s", s.to_string())])
    }
}

/// A short piece of `text` around the first match of `q` (lowercase), for the search results.
pub fn snippet(text: &str, q: &str) -> Option<String> {
    let low = text.to_lowercase();
    let i = low.find(q)?;
    // map the byte index of the lowercase text back onto the original by characters
    let ci = low[..i].chars().count();
    let chars: Vec<char> = text.chars().collect();
    let from = ci.saturating_sub(24);
    let to = (ci + q.chars().count() + 40).min(chars.len());
    let mut s: String = chars[from..to].iter().collect::<String>().replace('\n', " ");
    if from > 0 {
        s = format!("…{}", s.trim_start());
    }
    if to < chars.len() {
        s = format!("{}…", s.trim_end());
    }
    Some(s)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Puts a file that `gio trash` moved to the home trash back where it was (the newest copy). True when it worked.
pub fn restore_from_trash(original: &Path) -> bool {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"));
    restore_from(&data.join("Trash"), original)
}

/// Same as [`restore_from_trash`], from a given trash folder (with `info/` and `files/`).
fn restore_from(trash: &Path, original: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(trash.join("info")) else { return false };
    let mut best: Option<(String, PathBuf)> = None;
    for e in rd.flatten() {
        let p = e.path();
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let mut path = None;
        let mut date = String::new();
        for l in text.lines() {
            if let Some(v) = l.strip_prefix("Path=") {
                path = Some(percent_decode(v));
            } else if let Some(v) = l.strip_prefix("DeletionDate=") {
                date = v.to_string();
            }
        }
        if path.as_deref().map(Path::new) == Some(original) && best.as_ref().is_none_or(|b| date > b.0) {
            best = Some((date, p));
        }
    }
    let Some((_, info)) = best else { return false };
    let Some(stem) = info.file_stem() else { return false };
    let file = trash.join("files").join(stem);
    if original.exists() {
        return false;
    }
    let moved = std::fs::rename(&file, original).is_ok() || (std::fs::copy(&file, original).is_ok() && std::fs::remove_file(&file).is_ok());
    if moved {
        let _ = std::fs::remove_file(&info);
    }
    moved
}

/// The technical line of a file: "WAV 48 kHz 24 bits" (read from the file).
fn format_of(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_uppercase();
    if ext == "WAV" {
        if let Ok(r) = hound::WavReader::open(path) {
            let s = r.spec();
            return trf("WAV {khz} kHz {bits} bits", &[("khz", khz(s.sample_rate)), ("bits", s.bits_per_sample.to_string())]);
        }
    }
    if crate::media::have("ffprobe") {
        if let Ok(o) = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=codec_name,sample_rate", "-of", "default=nw=1"])
            .arg(path)
            .output()
        {
            let out = String::from_utf8_lossy(&o.stdout);
            let get = |k: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
            let rate: u32 = get("sample_rate").parse().unwrap_or(0);
            let codec = get("codec_name").to_uppercase();
            if rate > 0 {
                return format!("{} {} kHz", if codec.is_empty() { ext } else { codec }, khz(rate));
            }
        }
    }
    ext
}

fn khz(rate: u32) -> String {
    let k = rate as f64 / 1000.0;
    let s = if (k - k.round()).abs() < 0.01 { format!("{}", k.round()) } else { format!("{k:.1}") };
    if crate::i18n::system_locale().is_some() {
        s.replace('.', ",")
    } else {
        s
    }
}

// ---------------------------------------------------------------- the core's side

impl Core {
    pub fn is_favourite(&self, path: &Path) -> bool {
        let k = path.to_string_lossy();
        self.store.favourites.iter().any(|f| *f == k)
    }

    pub fn toggle_favourite(&mut self, path: &Path) {
        let k = path.to_string_lossy().to_string();
        if let Some(i) = self.store.favourites.iter().position(|f| *f == k) {
            self.store.favourites.remove(i);
        } else {
            self.store.favourites.push(k);
        }
    }

    /// Renames a recording (its transcript and subtitles go with it, and so do its tags and favourite mark).
    pub fn rename_recording(&mut self, path: &Path, new_stem: &str) -> Result<PathBuf, String> {
        let new_stem = new_stem.trim();
        if new_stem.is_empty() || new_stem.contains('/') {
            return Err(tr("That name can’t be used.").to_string());
        }
        let ext = path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let target = path.with_file_name(if ext.is_empty() { new_stem.to_string() } else { format!("{new_stem}.{ext}") });
        if target == path {
            return Ok(target);
        }
        if target.exists() {
            return Err(tr("A file with that name already exists.").to_string());
        }
        std::fs::rename(path, &target).map_err(|e| e.to_string())?;
        for side in ["txt", "srt"] {
            let a = PathBuf::from(format!("{}.{side}", path.display()));
            if a.is_file() {
                let _ = std::fs::rename(&a, format!("{}.{side}", target.display()));
            }
            let b = path.with_extension(side);
            if b.is_file() {
                let _ = std::fs::rename(&b, target.with_extension(side));
            }
        }
        let tags = self.tags_of(path);
        self.set_tags(path, vec![]);
        self.set_tags(&target, tags);
        if self.is_favourite(path) {
            self.toggle_favourite(path);
            self.toggle_favourite(&target);
        }
        self.scan_recordings();
        Ok(target)
    }
}

// ---------------------------------------------------------------- the page

#[derive(Clone, PartialEq)]
enum Filter {
    All,
    Transcript,
    Week,
    Tag(String),
    Favourites,
}

struct Play {
    path: PathBuf,
    player: Option<Player>,
    /// where it stopped (when paused) or started
    pos: f64,
    dur: f64,
}

/// The parts of the open detail that follow playback.
struct Detail {
    path: PathBuf,
    wave: gtk::DrawingArea,
    pos: gtk::Label,
    play: gtk::Button,
    lines: Vec<Line>,
    rows: Vec<gtk::Box>,
    scroller: gtk::ScrolledWindow,
    current: Cell<Option<usize>>,
    opened: Instant,
    /// while dragging the playhead: the fraction under the pointer
    drag: Cell<Option<f64>>,
}

struct Page {
    root: gtk::Box,
    list_side: gtk::Box,
    list: gtk::Box,
    empty: gtk::Box,
    subtitle: gtk::Label,
    chips: gtk::Box,
    search: gtk::Entry,
    filter: RefCell<Filter>,
    selected: RefCell<Option<PathBuf>>,
    /// medium/narrow: the detail is showing as its own page
    detail_open: Cell<bool>,
    side: gtk::Box,
    detail: RefCell<Option<Rc<Detail>>>,
    play: RefCell<Option<Play>>,
    waves: RefCell<Vec<(PathBuf, gtk::DrawingArea)>>,
    play_buttons: RefCell<Vec<(PathBuf, gtk::Button)>>,
    durs: RefCell<HashMap<PathBuf, f64>>,
    texts: RefCell<HashMap<PathBuf, (u64, String)>>,
    formats: RefCell<HashMap<PathBuf, String>>,
}

impl Page {
    fn dur_of(&self, r: &RecFile) -> Option<f64> {
        if let Some(d) = r.dur {
            return Some(d);
        }
        if let Some(d) = self.durs.borrow().get(&r.path) {
            return Some(*d);
        }
        let d = crate::media::duration(&r.path)?;
        self.durs.borrow_mut().insert(r.path.clone(), d);
        Some(d)
    }

    /// The transcript's text (cached while the file doesn't change).
    fn transcript_text(&self, r: &RecFile) -> Option<String> {
        let t = r.transcript()?;
        let m = std::fs::metadata(&t).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        if let Some((mm, s)) = self.texts.borrow().get(&t) {
            if *mm == m {
                return Some(s.clone());
            }
        }
        let s = std::fs::read_to_string(&t).ok()?;
        self.texts.borrow_mut().insert(t, (m, s.clone()));
        Some(s)
    }

    fn format_of(&self, p: &Path) -> String {
        if let Some(f) = self.formats.borrow().get(p) {
            return f.clone();
        }
        let f = format_of(p);
        self.formats.borrow_mut().insert(p.to_path_buf(), f.clone());
        f
    }

    /// Playback position of `path` as a fraction, when it is the one playing or paused.
    fn played(&self, path: &Path) -> Option<f64> {
        let p = self.play.borrow();
        let p = p.as_ref().filter(|p| p.path == path)?;
        Some((self.position_of(p) / p.dur.max(0.001)).clamp(0.0, 1.0))
    }

    fn position_of(&self, p: &Play) -> f64 {
        p.player.as_ref().map(|pl| pl.position()).unwrap_or(p.pos).min(p.dur)
    }

    fn position(&self) -> Option<(PathBuf, f64, bool)> {
        let p = self.play.borrow();
        let p = p.as_ref()?;
        Some((p.path.clone(), self.position_of(p), p.player.is_some()))
    }

    fn is_playing(&self, path: &Path) -> bool {
        self.play.borrow().as_ref().is_some_and(|p| p.path == path && p.player.is_some())
    }

    /// Play or pause `path` (starting where it was left).
    fn toggle_play(&self, path: &Path, dur: f64) {
        let mut slot = self.play.borrow_mut();
        match slot.as_mut() {
            Some(p) if p.path == path => {
                if let Some(pl) = p.player.take() {
                    p.pos = pl.position().min(p.dur);
                } else {
                    if p.pos >= p.dur - 0.05 {
                        p.pos = 0.0;
                    }
                    p.player = Some(Player::start(path, p.pos));
                }
            }
            _ => {
                *slot = Some(Play { path: path.to_path_buf(), player: Some(Player::start(path, 0.0)), pos: 0.0, dur: dur.max(0.1) });
            }
        }
        drop(slot);
        self.sync_buttons();
    }

    /// Moves playback of `path` to `t` seconds (keeps playing or paused).
    fn seek(&self, path: &Path, t: f64, dur: f64) {
        let mut slot = self.play.borrow_mut();
        let t = t.clamp(0.0, dur.max(0.0));
        match slot.as_mut() {
            Some(p) if p.path == path => {
                p.pos = t;
                if p.player.is_some() {
                    p.player = None;
                    p.player = Some(Player::start(path, t));
                }
            }
            _ => *slot = Some(Play { path: path.to_path_buf(), player: None, pos: t, dur: dur.max(0.1) }),
        }
        drop(slot);
        self.redraw_waves();
    }

    fn stop(&self) {
        self.play.borrow_mut().take();
        self.sync_buttons();
    }

    fn sync_buttons(&self) {
        for (p, b) in self.play_buttons.borrow().iter() {
            let on = self.is_playing(p);
            set_play_glyph(b, on, 14);
        }
        if let Some(d) = self.detail.borrow().as_ref() {
            set_play_glyph(&d.play, self.is_playing(&d.path), 18);
        }
        self.redraw_waves();
    }

    fn redraw_waves(&self) {
        for (_, w) in self.waves.borrow().iter() {
            w.queue_draw();
        }
        if let Some(d) = self.detail.borrow().as_ref() {
            d.wave.queue_draw();
        }
    }
}

fn set_play_glyph(b: &gtk::Button, playing: bool, px: i32) {
    if playing {
        b.add_css_class("tocando");
    } else {
        b.remove_css_class("tocando");
    }
    let token = if playing { "accent_fg" } else { "text" };
    b.set_child(Some(&icons::icon(if playing { "pausa" } else { "tocar" }, token, px)));
    b.set_tooltip_text(Some(if playing { tr("Pause") } else { tr("Play") }));
}

fn now_midnight() -> (u64, glib::DateTime) {
    let now = glib::DateTime::now_local().unwrap_or_else(|_| glib::DateTime::from_unix_utc(0).unwrap());
    let secs = now.to_unix() as u64;
    let into = (now.hour() * 3600 + now.minute() * 60 + now.second()) as u64;
    (secs.saturating_sub(into), now)
}

fn local(epoch: u64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(epoch as i64).ok()
}

fn hm(epoch: u64) -> String {
    local(epoch).map(|t| format!("{:02}:{:02}", t.hour(), t.minute())).unwrap_or_default()
}

fn group_title(g: &Group) -> String {
    match g {
        Group::Today => tr("Today").to_string(),
        Group::Yesterday => tr("Yesterday").to_string(),
        Group::ThisWeek => tr("This week").to_string(),
        Group::Date(d, m, y) => format!("{d:02}/{m:02}/{y}"),
    }
}

fn session_name(c: &Core, r: &RecFile) -> String {
    c.session_of_recording(r).map(|s| s.name.clone()).unwrap_or_else(|| {
        Path::new(&r.folder).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    })
}

/// "~/Música/Kalango Audio"
fn tilde(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match Path::new(p).strip_prefix(&home) {
        Ok(rest) if !home.is_empty() => format!("~/{}", rest.display()),
        _ => p.to_string(),
    }
}

/// Which recordings show, after the filter chip and the search.
fn visible(ui: &Rc<Ui>, page: &Rc<Page>) -> Vec<(RecFile, Option<String>)> {
    let c = ui.core.borrow();
    let q = page.search.text().to_string().trim().to_lowercase();
    let filter = page.filter.borrow().clone();
    let week_ago = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).saturating_sub(7 * 86_400);
    let mut out = vec![];
    for r in &c.recs {
        let tags = c.tags_of(&r.path);
        let ok = match &filter {
            Filter::All => true,
            Filter::Transcript => r.transcript().is_some(),
            Filter::Week => r.modified >= week_ago,
            Filter::Tag(t) => tags.contains(t),
            Filter::Favourites => c.is_favourite(&r.path),
        };
        if !ok {
            continue;
        }
        let mut hit = None;
        if !q.is_empty() {
            let name_hit = r.name.to_lowercase().contains(&q) || session_name(&c, r).to_lowercase().contains(&q);
            let tag_hit = tags.iter().any(|t| t.to_lowercase().contains(&q));
            if !name_hit && !tag_hit {
                match page.transcript_text(r).and_then(|t| snippet(&t, &q)) {
                    Some(s) => hit = Some(s),
                    None => continue,
                }
            }
        }
        out.push((r.clone(), hit));
    }
    out
}

fn more_menu(ui: &Rc<Ui>, page: &Rc<Page>, path: &Path) -> gtk::MenuButton {
    let more = gtk::MenuButton::new();
    more.add_css_class("ka-icone");
    more.set_child(Some(&icons::icon("pontos-v", "muted", 16)));
    more.set_valign(Align::Center);
    more.set_tooltip_text(Some(tr("More")));
    let pop = gtk::Popover::new();
    pop.add_css_class("ka-grav-menu");
    let items = vbox(2, "");
    let fav = ui.core.borrow().is_favourite(path);
    let entries: [(&str, &str, u8); 5] = [
        ("pasta", tr("Show in Files"), 0),
        ("ajustes", tr("Open in editor"), 1),
        ("lapis", tr("Rename…"), 2),
        ("estrela-n", if fav { tr("Remove from favourites") } else { tr("Add to favourites") }, 3),
        ("lixeira", tr("Move to the trash"), 4),
    ];
    for (glyph, text, what) in entries {
        let b = gtk::Button::new();
        b.add_css_class("ka-grav-item");
        let row = hbox(10, "");
        row.append(&icons::icon(glyph, if what == 4 { "red" } else { "text2" }, 14));
        row.append(&label(text, "ka-texto"));
        b.set_child(Some(&row));
        let (u, p, pg, path) = (ui.clone(), pop.clone(), page.clone(), path.to_path_buf());
        b.connect_clicked(move |_| {
            p.popdown();
            match what {
                0 => show_in_files(&path),
                1 => open_in_editor(&u, &path),
                2 => rename_dialog(&u, &pg, &path),
                3 => {
                    u.core.borrow_mut().toggle_favourite(&path);
                    u.changed(Change::Store);
                }
                _ => trash(&u, &pg, &path),
            }
        });
        items.append(&b);
    }
    pop.set_child(Some(&items));
    more.set_popover(Some(&pop));
    more
}

fn show_in_files(path: &Path) {
    if let Some(d) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(d).spawn();
    }
}

fn open_in_editor(ui: &Rc<Ui>, path: &Path) {
    let ed = ui.core.borrow().store.prefs.audio_editor.trim().to_string();
    if ed.is_empty() {
        ui.go("ajustes");
        ui.toast(tr("Choose the audio editor that opens recordings."), None);
        return;
    }
    if std::process::Command::new(&ed).arg(path).spawn().is_err() {
        ui.toast(&trf("Could not open “{app}”.", &[("app", ed)]), None);
    }
}

fn trash(ui: &Rc<Ui>, page: &Rc<Page>, path: &Path) {
    if page.play.borrow().as_ref().is_some_and(|p| p.path == path) {
        page.stop();
    }
    let (tags, fav, transcript) = {
        let c = ui.core.borrow();
        let t = c.recs.iter().find(|r| r.path == path).and_then(|r| r.transcript());
        (c.tags_of(path), c.is_favourite(path), t)
    };
    let ok = ui.core.borrow_mut().trash_recording(path);
    if !ok {
        ui.toast(tr("Could not move the recording to the trash."), None);
        return;
    }
    if page.selected.borrow().as_deref() == Some(path) {
        page.selected.borrow_mut().take();
        page.detail_open.set(false);
    }
    ui.changed(Change::Recordings);
    let path = path.to_path_buf();
    ui.toast(
        tr("Moved to the trash"),
        Some((
            tr("Undo"),
            Box::new(move |u: &Rc<Ui>| {
                if restore_from_trash(&path) {
                    if let Some(t) = &transcript {
                        restore_from_trash(t);
                    }
                    {
                        let mut c = u.core.borrow_mut();
                        c.set_tags(&path, tags.clone());
                        if fav && !c.is_favourite(&path) {
                            c.toggle_favourite(&path);
                        }
                        c.scan_recordings();
                    }
                    u.changed(Change::Recordings);
                } else {
                    u.toast(tr("Could not take it back out of the trash; it is still there."), None);
                }
            }),
        )),
    );
}

/// A dialog over the window: the veil, the box in the middle; Esc or a click outside closes it. Returns the closer.
pub(super) fn modal(ui: &Rc<Ui>, content: &gtk::Box, width: i32) -> Rc<dyn Fn()> {
    let veil = gtk::Box::new(gtk::Orientation::Vertical, 0);
    veil.add_css_class("ka-veu");
    veil.set_hexpand(true);
    veil.set_vexpand(true);
    content.add_css_class("ka-dialogo");
    content.set_size_request(width, -1);
    content.set_halign(Align::Center);
    content.set_valign(Align::Center);
    content.set_margin_start(16);
    content.set_margin_end(16);
    let anim = animated(content);
    anim.set_halign(Align::Center);
    anim.set_valign(Align::Center);
    anim.set_vexpand(true);
    veil.append(&anim);
    ui.overlay.add_overlay(&veil);
    appear(&anim, 0.0, 12.0);
    let overlay = ui.overlay.clone();
    let done = Rc::new(Cell::new(false));
    let close: Rc<dyn Fn()> = {
        let (veil, anim, done) = (veil.clone(), anim.clone(), done.clone());
        Rc::new(move || {
            if done.replace(true) {
                return;
            }
            let (o, v) = (overlay.clone(), veil.clone());
            anim.sair((0.0, 12.0), 1.0, move || o.remove_overlay(&v));
        })
    };
    let click = gtk::GestureClick::new();
    {
        let (close, content) = (close.clone(), content.clone());
        click.connect_pressed(move |g, _, x, y| {
            let Some(w) = g.widget() else { return };
            if let Some(p) = w.compute_point(&content, &gtk::graphene::Point::new(x as f32, y as f32)) {
                let (cw, ch) = (content.width() as f32, content.height() as f32);
                if p.x() < 0.0 || p.y() < 0.0 || p.x() > cw || p.y() > ch {
                    close();
                }
            }
        });
    }
    veil.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let close = close.clone();
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Escape {
                close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    veil.add_controller(keys);
    close
}

fn rename_dialog(ui: &Rc<Ui>, page: &Rc<Page>, path: &Path) {
    let d = vbox(12, "");
    d.append(&label(tr("Rename recording"), "ka-titulo2"));
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let e = entry(tr("Name"), None);
    e.set_text(&stem);
    d.append(&e);
    let err = label("", "ka-fraco cor-red");
    err.set_visible(false);
    d.append(&err);
    let row = hbox(8, "");
    row.set_halign(Align::End);
    let cancel = button(tr("Cancel"), "fantasma", None);
    let ok = button(tr("Rename"), "primario", None);
    row.append(&cancel);
    row.append(&ok);
    d.append(&row);
    let close = modal(ui, &d, 420);
    {
        let close = close.clone();
        cancel.connect_clicked(move |_| close());
    }
    let go = {
        let (u, pg, path, e, err, close) = (ui.clone(), page.clone(), path.to_path_buf(), e.clone(), err.clone(), close.clone());
        Rc::new(move || {
            if pg.play.borrow().as_ref().is_some_and(|p| p.path == path) {
                pg.stop();
            }
            let r = u.core.borrow_mut().rename_recording(&path, &e.text());
            match r {
                Ok(new) => {
                    if pg.selected.borrow().as_deref() == Some(path.as_path()) {
                        *pg.selected.borrow_mut() = Some(new);
                    }
                    close();
                    u.changed(Change::Recordings);
                }
                Err(m) => {
                    err.set_text(&m);
                    err.set_visible(true);
                }
            }
        })
    };
    {
        let go = go.clone();
        ok.connect_clicked(move |_| go());
    }
    e.connect_activate(move |_| go());
    e.grab_focus();
}

fn row(ui: &Rc<Ui>, page: &Rc<Page>, r: &RecFile, hit: Option<&str>, narrow: bool, midnight: u64) -> gtk::Widget {
    let (sess, tags) = {
        let c = ui.core.borrow();
        (session_name(&c, r), c.tags_of(&r.path))
    };
    let dur = page.dur_of(r);
    let line_box = hbox(16, "ka-grav-linha");
    if page.selected.borrow().as_deref() == Some(r.path.as_path()) {
        line_box.add_css_class("selecionado");
    }
    let play = gtk::Button::new();
    play.add_css_class("ka-grav-tocar");
    play.set_valign(Align::Center);
    set_play_glyph(&play, page.is_playing(&r.path), 14);
    {
        let (pg, path) = (page.clone(), r.path.clone());
        let d = dur.unwrap_or(0.0);
        let u = ui.clone();
        play.connect_clicked(move |_| {
            pg.toggle_play(&path, d);
            // in a wide window the row that plays is the one shown on the right
            if u.layout() == Layout::Wide && pg.selected.borrow().as_deref() != Some(path.as_path()) {
                select(&u, &pg, &path);
            }
        });
    }
    page.play_buttons.borrow_mut().push((r.path.clone(), play.clone()));
    line_box.append(&play);

    let names = vbox(2, "");
    let stem = r.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    names.append(&line(&display_name(&stem), "ka-titulo3"));
    let when = if days_ago(r.modified, midnight) <= 1 { hm(r.modified) } else { super::inicio::when_label(r.modified) };
    names.append(&line(&format!("{sess} · {when}"), "ka-fraco"));
    if let Some(h) = hit {
        names.append(&line(h, "ka-fraco ka-grav-trecho"));
    }
    names.set_valign(Align::Center);
    if narrow {
        names.set_hexpand(true);
    } else {
        names.set_size_request(220, -1);
        names.set_hexpand(false);
        for c in [names.first_child(), names.last_child()].into_iter().flatten() {
            c.set_hexpand(false);
        }
    }
    line_box.append(&names);

    if !narrow {
        let pg = page.clone();
        let path = r.path.clone();
        let w = wave(&r.path, 216, 32, "muted", move || pg.played(&path));
        w.set_valign(Align::Center);
        page.waves.borrow_mut().push((r.path.clone(), w.clone()));
        line_box.append(&w);
    }
    let dl = label(&dur.map(fmt_duration).unwrap_or_default(), "ka-texto2 ka-tnum");
    dl.set_xalign(1.0);
    dl.set_size_request(56, -1);
    line_box.append(&dl);
    if !narrow {
        let sl = label(&fmt_size(r.size), "ka-fraco ka-tnum");
        sl.set_xalign(1.0);
        sl.set_size_request(56, -1);
        line_box.append(&sl);
        let tb = hbox(6, "");
        tb.set_hexpand(true);
        tb.set_valign(Align::Center);
        for (i, t) in tags.iter().take(3).enumerate() {
            tb.append(&chip(t, if i == 0 { "accent" } else { "teal" }));
        }
        line_box.append(&tb);
    }
    let tr_icon = icons::icon("doc", "accent", 16);
    tr_icon.set_opacity(if r.transcript().is_some() { 1.0 } else { 0.0 });
    if r.transcript().is_some() {
        tr_icon.set_tooltip_text(Some(tr("Has a transcript")));
    }
    line_box.append(&tr_icon);
    line_box.append(&more_menu(ui, page, &r.path));

    let click = gtk::GestureClick::new();
    {
        let (u, pg, path) = (ui.clone(), page.clone(), r.path.clone());
        click.connect_released(move |g, _, _, _| {
            // the buttons inside the row handle their own clicks
            if let Some(w) = g.widget() {
                if let Some(target) = w.pick(g.point(None).map(|p| p.0).unwrap_or(0.0), g.point(None).map(|p| p.1).unwrap_or(0.0), gtk::PickFlags::DEFAULT) {
                    if target.ancestor(gtk::Button::static_type()).is_some() || target.is::<gtk::Button>() {
                        return;
                    }
                }
            }
            select(&u, &pg, &path);
        });
    }
    line_box.add_controller(click);
    line_box.set_cursor_from_name(Some("pointer"));
    line_box.upcast()
}

fn select(ui: &Rc<Ui>, page: &Rc<Page>, path: &Path) {
    *page.selected.borrow_mut() = Some(path.to_path_buf());
    page.detail_open.set(true);
    refresh_list(ui, page, false);
    refresh_detail(ui, page);
    arrange(ui, page);
}

fn refresh_header(ui: &Rc<Ui>, page: &Rc<Page>) {
    let (n, size, folder, tags) = {
        let c = ui.core.borrow();
        let mut tags: Vec<String> = vec![];
        for r in &c.recs {
            for t in c.tags_of(&r.path) {
                if !tags.contains(&t) {
                    tags.push(t);
                }
            }
        }
        (c.recs.len(), c.recs.iter().map(|r| r.size).sum::<u64>(), c.store.prefs.default_folder.clone(), tags)
    };
    let gb = size as f64 / 1_073_741_824.0;
    let total = if gb >= 1.0 {
        let s = format!("{gb:.1} GB");
        if crate::i18n::system_locale().is_some() {
            s.replace('.', ",")
        } else {
            s
        }
    } else {
        fmt_size(size)
    };
    page.subtitle.set_text(&trf(
        "{n} recordings · {size} in {folder}",
        &[("n", n.to_string()), ("size", total), ("folder", tilde(&folder))],
    ));
    // the chips: fixed ones, the tags in use, favourites
    clear(&page.chips);
    let current = page.filter.borrow().clone();
    if let Filter::Tag(t) = &current {
        if !tags.contains(t) {
            *page.filter.borrow_mut() = Filter::All;
        }
    }
    let current = page.filter.borrow().clone();
    let mut list = vec![(tr("All").to_string(), Filter::All), (tr("With transcript").to_string(), Filter::Transcript), (tr("This week").to_string(), Filter::Week)];
    list.extend(tags.into_iter().map(|t| (t.clone(), Filter::Tag(t))));
    list.push((tr("Favourites").to_string(), Filter::Favourites));
    let mut first: Option<gtk::ToggleButton> = None;
    for (text, f) in list {
        let b = gtk::ToggleButton::with_label(&text);
        b.add_css_class("ka-filtro");
        if let Some(g) = &first {
            b.set_group(Some(g));
        } else {
            first = Some(b.clone());
        }
        b.set_active(f == current);
        let (u, pg) = (ui.clone(), page.clone());
        b.connect_toggled(move |b| {
            if b.is_active() && *pg.filter.borrow() != f {
                *pg.filter.borrow_mut() = f.clone();
                refresh_list(&u, &pg, true);
            }
        });
        page.chips.append(&b);
    }
}

fn refresh_list(ui: &Rc<Ui>, page: &Rc<Page>, animate: bool) {
    clear(&page.list);
    page.waves.borrow_mut().clear();
    page.play_buttons.borrow_mut().clear();
    let total = ui.core.borrow().recs.len();
    page.empty.set_visible(total == 0);
    page.list.set_visible(total > 0);
    page.chips.set_visible(total > 0);
    let items = visible(ui, page);
    let narrow = ui.layout() == Layout::Narrow || (ui.layout() == Layout::Medium && false);
    let (midnight, _) = now_midnight();
    let mut last: Option<Group> = None;
    let mut anims = vec![];
    if items.is_empty() && total > 0 {
        let none = label(tr("Nothing matches the search or the filter."), "ka-fraco");
        none.set_margin_top(24);
        page.list.append(&none);
    }
    for (r, hit) in &items {
        let days = days_ago(r.modified, midnight);
        let date = local(r.modified).map(|t| (t.day_of_month() as u32, t.month() as u32, t.year())).unwrap_or((1, 1, 1970));
        let g = group_of(days, date);
        if last.as_ref() != Some(&g) {
            let cap = caps(&group_title(&g));
            cap.set_margin_top(if last.is_none() { 4 } else { 12 });
            cap.set_margin_bottom(8);
            page.list.append(&cap);
            last = Some(g);
        }
        let w = row(ui, page, r, hit.as_deref(), narrow, midnight);
        let a = animated(&w);
        page.list.append(&a);
        anims.push(a);
    }
    if animate && page.list.is_mapped() {
        cascade(&anims);
    }
}

fn transport_button(glyph: &str, tip: &str) -> gtk::Button {
    let b = icon_button(glyph, "text", 14, tip);
    b.add_css_class("ka-grav-transporte");
    b
}

fn refresh_detail(ui: &Rc<Ui>, page: &Rc<Page>) {
    clear(&page.side);
    page.detail.borrow_mut().take();
    let sel = page.selected.borrow().clone();
    let rec = sel.and_then(|p| ui.core.borrow().recs.iter().find(|r| r.path == p).cloned());
    let Some(r) = rec else {
        let v = vbox(8, "");
        v.set_valign(Align::Center);
        v.set_vexpand(true);
        let i = icons::icon("musica", "dim", 28);
        v.append(&i);
        let l = label(tr("Choose a recording to see its waveform, tags and transcript."), "ka-fraco");
        l.set_xalign(0.5);
        l.set_wrap(true);
        v.append(&l);
        page.side.append(&v);
        return;
    };
    let dur = page.dur_of(&r).unwrap_or(0.0);
    let narrow_page = ui.layout() != Layout::Wide;

    let top = vbox(0, "");
    if narrow_page {
        let back = button(tr("Recordings"), "fantasma", Some("seta-esquerda"));
        back.set_halign(Align::Start);
        back.set_margin_bottom(12);
        let (u, pg) = (ui.clone(), page.clone());
        back.connect_clicked(move |_| {
            pg.detail_open.set(false);
            arrange(&u, &pg);
        });
        top.append(&back);
    }
    let stem = r.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let title_row = hbox(8, "");
    title_row.append(&line(&display_name(&stem), "ka-grav-titulo"));
    let fav = ui.core.borrow().is_favourite(&r.path);
    let star = icon_button("estrela-n", if fav { "gold" } else { "dim" }, 16, if fav { tr("Remove from favourites") } else { tr("Add to favourites") });
    {
        let (u, path) = (ui.clone(), r.path.clone());
        star.connect_clicked(move |_| {
            u.core.borrow_mut().toggle_favourite(&path);
            u.changed(Change::Store);
        });
    }
    title_row.append(&star);
    top.append(&title_row);
    let when = {
        let (midnight, _) = now_midnight();
        let d = days_ago(r.modified, midnight);
        let date = local(r.modified).map(|t| (t.day_of_month() as u32, t.month() as u32, t.year())).unwrap_or((1, 1, 1970));
        format!("{} {}", group_title(&group_of(d.min(if d <= 1 { d } else { 7 }), date)), hm(r.modified))
    };
    let meta = [when, long_duration(dur), page.format_of(&r.path), fmt_size(r.size)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    let ml = line(&meta, "ka-fraco");
    ml.set_margin_top(4);
    top.append(&ml);

    // the big waveform with the playhead: click or drag to seek
    let big = gtk::DrawingArea::new();
    big.set_content_height(84);
    big.set_hexpand(true);
    big.set_margin_top(20);
    big.add_css_class("ka-onda");
    big.set_cursor_from_name(Some("pointer"));
    top.append(&big);
    let times = hbox(0, "");
    times.set_margin_top(6);
    let pos = label("0:00", "ka-fraco ka-tnum");
    times.append(&pos);
    times.append(&spacer());
    times.append(&label(&fmt_duration(dur), "ka-fraco ka-tnum"));
    top.append(&times);

    // transport: −15 s, previous, play/pause, next, +15 s (speed: the player plays at 1× only)
    let tp = hbox(10, "");
    tp.set_halign(Align::Center);
    tp.set_margin_top(8);
    let back15 = button(tr("−15 s"), "fantasma", None);
    let prev = transport_button("anterior", tr("Previous recording"));
    let play = gtk::Button::new();
    play.add_css_class("ka-grav-tocar-grande");
    set_play_glyph(&play, page.is_playing(&r.path), 18);
    let next = transport_button("proxima", tr("Next recording"));
    let fwd15 = button(tr("+15 s"), "fantasma", None);
    for w in [back15.upcast_ref::<gtk::Widget>(), prev.upcast_ref(), play.upcast_ref(), next.upcast_ref(), fwd15.upcast_ref()] {
        tp.append(w);
    }
    top.append(&tp);

    // tags
    let tcap = caps(tr("Tags"));
    tcap.set_margin_top(20);
    tcap.set_margin_bottom(8);
    top.append(&tcap);
    let tags_box = gtk::FlowBox::new();
    tags_box.set_selection_mode(gtk::SelectionMode::None);
    tags_box.set_column_spacing(6);
    tags_box.set_row_spacing(6);
    tags_box.set_max_children_per_line(12);
    tags_box.add_css_class("ka-grav-etiquetas");
    let tags = ui.core.borrow().tags_of(&r.path);
    for (i, t) in tags.iter().enumerate() {
        let b = gtk::Button::new();
        b.add_css_class("ka-grav-etiqueta");
        let c = chip(t, if i == 0 { "accent" } else { "teal" });
        b.set_child(Some(&c));
        b.set_halign(Align::Start);
        b.set_tooltip_text(Some(&trf("Remove the tag “{tag}”", &[("tag", t.clone())])));
        let (u, path, t) = (ui.clone(), r.path.clone(), t.clone());
        b.connect_clicked(move |_| {
            let mut v = u.core.borrow().tags_of(&path);
            v.retain(|x| *x != t);
            u.core.borrow_mut().set_tags(&path, v);
            u.changed(Change::Store);
        });
        tags_box.append(&b);
    }
    let add = gtk::Entry::new();
    add.add_css_class("ka-grav-nova-etiqueta");
    add.set_placeholder_text(Some(tr("+ tag")));
    add.set_width_chars(8);
    add.set_max_width_chars(14);
    add.set_halign(Align::Start);
    {
        let (u, path) = (ui.clone(), r.path.clone());
        add.connect_activate(move |e| {
            let t = e.text().trim().to_string();
            if t.is_empty() {
                return;
            }
            let mut v = u.core.borrow().tags_of(&path);
            if !v.contains(&t) {
                v.push(t);
            }
            u.core.borrow_mut().set_tags(&path, v);
            u.changed(Change::Store);
        });
    }
    tags_box.append(&add);
    top.append(&tags_box);

    // the transcript
    let text = page.transcript_text(&r);
    let lines = text.as_deref().map(parse_transcript).unwrap_or_default();
    let thead = hbox(0, "");
    thead.set_margin_top(20);
    thead.set_margin_bottom(6);
    thead.append(&caps(tr("Transcript")));
    thead.append(&spacer());
    if text.is_some() {
        thead.append(&label(tr("Made on this computer"), "ka-fraco2"));
    }
    top.append(&thead);
    page.side.append(&top);

    let tlist = vbox(8, "");
    let mut rows = vec![];
    for l in &lines {
        let rw = hbox(12, "ka-grav-fala");
        let t = label(&l.at.map(fmt_stamp).unwrap_or_default(), "ka-fraco ka-tnum");
        t.set_size_request(40, -1);
        t.set_valign(Align::Start);
        rw.append(&t);
        let body = vbox(2, "");
        body.set_hexpand(true);
        if !l.speaker.is_empty() {
            body.append(&label(&l.speaker, "ka-grav-quem"));
        }
        body.append(&para(&l.text, "ka-texto2"));
        rw.append(&body);
        if let Some(at) = l.at {
            let click = gtk::GestureClick::new();
            let (pg, path) = (page.clone(), r.path.clone());
            click.connect_released(move |_, _, _, _| pg.seek(&path, at, dur));
            rw.add_controller(click);
            rw.set_cursor_from_name(Some("pointer"));
        }
        tlist.append(&rw);
        rows.push(rw);
    }
    if lines.is_empty() {
        let none = para(tr("No transcript yet. A Transcribe block or the “Transcribe when done” automation writes one next to the recording."), "ka-fraco");
        tlist.append(&none);
    }
    let tsc = scroller(&tlist);
    tsc.set_vexpand(true);
    tsc.set_min_content_height(120);
    page.side.append(&tsc);

    let div = hbox(0, "ka-linha-div");
    div.set_margin_top(12);
    page.side.append(&div);
    let acts1 = hbox(8, "");
    acts1.set_margin_top(12);
    let copy = button(tr("Copy text"), "secundario", Some("copiar"));
    let srt = button(tr("Export .srt"), "secundario", Some("baixar"));
    let sum = button(tr("Summarize with the agent"), "fantasma", Some("spark"));
    for b in [&copy, &srt] {
        b.set_sensitive(!lines.is_empty());
    }
    acts1.append(&copy);
    acts1.append(&srt);
    acts1.append(&sum);
    page.side.append(&acts1);
    let acts2 = hbox(8, "");
    acts2.set_margin_top(8);
    let files = button(tr("Show in Files"), "fantasma", Some("pasta"));
    let editor = button(tr("Open in editor"), "fantasma", Some("ajustes"));
    let del = button(tr("Delete"), "fantasma", Some("lixeira"));
    acts2.append(&files);
    acts2.append(&editor);
    acts2.append(&del);
    page.side.append(&acts2);
    let note = para(tr("Delete moves the recording to the Trash; you can undo it right after."), "ka-fraco2");
    note.set_margin_top(12);
    page.side.append(&note);

    // wiring
    let detail = Rc::new(Detail {
        path: r.path.clone(),
        wave: big.clone(),
        pos: pos.clone(),
        play: play.clone(),
        lines: lines.clone(),
        rows,
        scroller: tsc.clone(),
        current: Cell::new(None),
        opened: Instant::now(),
        drag: Cell::new(None),
    });
    {
        let (pg, path, d) = (page.clone(), r.path.clone(), Rc::downgrade(&detail));
        big.set_draw_func(move |a, cr, w, h| {
            let Some(p) = peaks_for(&path, a) else { return };
            let Some(d) = d.upgrade() else { return };
            // the wave comes in from the left in 180 ms
            let k = (d.opened.elapsed().as_secs_f64() / 0.18).min(1.0);
            cr.rectangle(0.0, 0.0, w as f64 * k, h as f64);
            cr.clip();
            let f = d.drag.get().or_else(|| pg.played(&path)).unwrap_or(0.0);
            draw_wave(cr, w as f64, h as f64, &p, Some(f), "accent", "dim");
        });
        follow_theme(&big);
    }
    {
        let drag = gtk::GestureDrag::new();
        let (pg, path, d) = (page.clone(), r.path.clone(), Rc::downgrade(&detail));
        let start = Rc::new(Cell::new(0.0f64));
        {
            let (start, d, big) = (start.clone(), d.clone(), big.clone());
            drag.connect_drag_begin(move |_, x, _| {
                start.set(x);
                if let Some(d) = d.upgrade() {
                    d.drag.set(Some((x / big.width().max(1) as f64).clamp(0.0, 1.0)));
                    big.queue_draw();
                }
            });
        }
        {
            let (start, d, big) = (start.clone(), d.clone(), big.clone());
            drag.connect_drag_update(move |_, dx, _| {
                if let Some(d) = d.upgrade() {
                    d.drag.set(Some(((start.get() + dx) / big.width().max(1) as f64).clamp(0.0, 1.0)));
                    d.pos.set_text(&fmt_duration(d.drag.get().unwrap_or(0.0) * dur));
                    big.queue_draw();
                }
            });
        }
        drag.connect_drag_end(move |_, _, _| {
            if let Some(d) = d.upgrade() {
                if let Some(f) = d.drag.take() {
                    pg.seek(&path, f * dur, dur);
                }
            }
        });
        big.add_controller(drag);
    }
    {
        let (pg, path) = (page.clone(), r.path.clone());
        play.connect_clicked(move |_| pg.toggle_play(&path, dur));
    }
    for (b, delta) in [(&back15, -15.0), (&fwd15, 15.0)] {
        let (pg, path) = (page.clone(), r.path.clone());
        b.connect_clicked(move |_| {
            let now = pg.position().filter(|p| p.0 == path).map(|p| p.1).unwrap_or(0.0);
            pg.seek(&path, now + delta, dur);
        });
    }
    for (b, step) in [(&prev, -1i32), (&next, 1)] {
        let (u, pg, path) = (ui.clone(), page.clone(), r.path.clone());
        b.connect_clicked(move |_| {
            let list: Vec<PathBuf> = visible(&u, &pg).into_iter().map(|x| x.0.path).collect();
            let Some(i) = list.iter().position(|p| *p == path) else { return };
            let j = i as i32 + step;
            if j >= 0 && (j as usize) < list.len() {
                let was_playing = pg.is_playing(&path);
                let to = list[j as usize].clone();
                select(&u, &pg, &to);
                if was_playing {
                    let d = u.core.borrow().recs.iter().find(|r| r.path == to).cloned().and_then(|r| pg.dur_of(&r)).unwrap_or(0.0);
                    pg.toggle_play(&to, d);
                }
            }
        });
    }
    {
        let (u, raw) = (ui.clone(), text.clone().unwrap_or_default());
        copy.connect_clicked(move |_| {
            u.window.clipboard().set_text(&raw);
            u.toast(tr("Transcript copied"), None);
        });
    }
    {
        let (u, path, lines) = (ui.clone(), r.path.clone(), lines.clone());
        srt.connect_clicked(move |_| {
            let out = path.with_extension("srt");
            match std::fs::write(&out, to_srt(&lines, Some(dur))) {
                Ok(()) => u.toast(&trf("Saved “{file}”", &[("file", out.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())]), None),
                Err(e) => u.toast(&trf("Could not save the subtitles: {err}", &[("err", e.to_string())]), None),
            }
        });
    }
    {
        let (u, name) = (ui.clone(), display_name(&stem));
        sum.connect_clicked(move |_| super::agente::ask(&u, &trf("summarize the recording “{name}”", &[("name", name.clone())])));
    }
    {
        let path = r.path.clone();
        files.connect_clicked(move |_| show_in_files(&path));
    }
    {
        let (u, path) = (ui.clone(), r.path.clone());
        editor.connect_clicked(move |_| open_in_editor(&u, &path));
    }
    {
        let (u, pg, path) = (ui.clone(), page.clone(), r.path.clone());
        del.connect_clicked(move |_| trash(&u, &pg, &path));
    }
    *page.detail.borrow_mut() = Some(detail);
    follow_playback(page);
}

/// The time, the highlighted line of the transcript (scrolled into view) and the playhead.
fn follow_playback(page: &Rc<Page>) {
    let Some(d) = page.detail.borrow().clone() else { return };
    let pos = page.position().filter(|p| p.0 == d.path).map(|p| p.1);
    if d.drag.get().is_none() {
        d.pos.set_text(&fmt_duration(pos.unwrap_or(0.0)));
    }
    let cur = pos.and_then(|p| current_line(&d.lines, p));
    if cur != d.current.get() {
        if let Some(old) = d.current.get().and_then(|i| d.rows.get(i)) {
            old.remove_css_class("atual");
        }
        if let Some(i) = cur {
            if let Some(rw) = d.rows.get(i) {
                rw.add_css_class("atual");
                // keep the current line in view: a third from the top
                if let Some(p) = rw.compute_point(&d.scroller.child().unwrap_or_else(|| rw.clone().upcast()), &gtk::graphene::Point::new(0.0, 0.0)) {
                    let adj = d.scroller.vadjustment();
                    let target = (p.y() as f64 - adj.page_size() / 3.0).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0));
                    let from = adj.value();
                    let mut spring = kalango_movimento::Mola::new(from);
                    spring.ir(target, kalango_movimento::SUAVE);
                    let spring = RefCell::new(spring);
                    let last = Cell::new(Instant::now());
                    d.scroller.add_tick_callback(move |_, _| {
                        let mut s = spring.borrow_mut();
                        s.passo(last.replace(Instant::now()).elapsed().as_secs_f64().min(0.05));
                        adj.set_value(s.x);
                        if s.parada() {
                            glib::ControlFlow::Break
                        } else {
                            glib::ControlFlow::Continue
                        }
                    });
                }
            }
        }
        d.current.set(cur);
    }
}

/// Wide: list and detail side by side. Medium/narrow: one or the other.
fn arrange(ui: &Rc<Ui>, page: &Rc<Page>) {
    let wide = ui.layout() == Layout::Wide;
    let has = page.selected.borrow().is_some();
    if wide {
        page.list_side.set_visible(true);
        page.side.set_visible(true);
        page.side.set_hexpand(false);
        page.side.set_size_request(470, -1);
        page.side.remove_css_class("ka-grav-pagina-detalhe");
    } else {
        let show_detail = has && page.detail_open.get();
        page.list_side.set_visible(!show_detail);
        page.side.set_visible(show_detail);
        page.side.set_hexpand(true);
        page.side.set_size_request(-1, -1);
        page.side.add_css_class("ka-grav-pagina-detalhe");
    }
    let _ = &page.root;
}

fn rescan(ui: &Rc<Ui>, page: &Rc<Page>, animate: bool) {
    ui.core.borrow_mut().scan_recordings();
    // keep the selection when the file is still there; in a wide window, pick the newest
    let first = {
        let c = ui.core.borrow();
        let still = page.selected.borrow().as_ref().is_some_and(|p| c.recs.iter().any(|r| r.path == *p));
        if !still {
            page.selected.borrow_mut().take();
        }
        // the newest that is not still being written (an open WAV has no length yet)
        c.recs.iter().find(|r| r.dur.is_none_or(|d| d > 0.0)).or(c.recs.first()).map(|r| r.path.clone())
    };
    if page.selected.borrow().is_none() && ui.layout() == Layout::Wide {
        *page.selected.borrow_mut() = first;
    }
    let gone = page.play.borrow().as_ref().is_some_and(|p| !p.path.exists());
    if gone {
        page.stop();
    }
    refresh_header(ui, page);
    refresh_list(ui, page, animate);
    refresh_detail(ui, page);
    arrange(ui, page);
}

pub fn page(ui: &Rc<Ui>) -> gtk::Widget {
    let root = hbox(0, "ka-gravacoes");
    let list_side = vbox(0, "ka-pagina ka-grav-lista");
    list_side.set_hexpand(true);

    // header: title and numbers, search, open folder
    let head = hbox(12, "");
    let titles = vbox(4, "");
    titles.set_hexpand(true);
    titles.append(&label(tr("Recordings"), "ka-titulo"));
    let subtitle = line("", "ka-fraco");
    titles.append(&subtitle);
    head.append(&titles);
    let search = entry(tr("Search by name, tag or speech"), Some("busca"));
    search.set_size_request(240, -1);
    search.set_valign(Align::Start);
    head.append(&search);
    let folder = button(tr("Open folder"), "secundario", Some("pasta"));
    folder.set_valign(Align::Start);
    head.append(&folder);
    list_side.append(&head);

    let chips = hbox(8, "ka-grav-filtros");
    chips.set_margin_top(16);
    chips.set_margin_bottom(16);
    let chips_sc = gtk::ScrolledWindow::new();
    chips_sc.set_vscrollbar_policy(gtk::PolicyType::Never);
    chips_sc.set_hscrollbar_policy(gtk::PolicyType::External);
    chips_sc.set_child(Some(&chips));
    list_side.append(&chips_sc);

    let list = vbox(4, "");
    let empty = vbox(10, "ka-grav-vazio");
    empty.set_valign(Align::Center);
    empty.set_halign(Align::Center);
    empty.set_vexpand(true);
    let et = tile("musica", "accent", 48);
    et.set_halign(Align::Center);
    empty.append(&et);
    let e1 = label(tr("No recordings yet"), "ka-titulo2");
    e1.set_xalign(0.5);
    empty.append(&e1);
    let e2 = label(tr("What your sessions record shows up here, with its waveform and transcript."), "ka-fraco");
    e2.set_xalign(0.5);
    e2.set_wrap(true);
    empty.append(&e2);
    let go_sessions = button(tr("Sessions"), "primario", Some("casa"));
    go_sessions.set_halign(Align::Center);
    empty.append(&go_sessions);
    let body = vbox(0, "");
    body.append(&list);
    body.append(&empty);
    let sc = scroller(&body);
    list_side.append(&sc);
    root.append(&list_side);

    let side = vbox(0, "ka-grav-detalhe");
    root.append(&side);

    let page = Rc::new(Page {
        root: root.clone(),
        list_side,
        list,
        empty,
        subtitle,
        chips,
        search: search.clone(),
        filter: RefCell::new(Filter::All),
        selected: RefCell::new(None),
        detail_open: Cell::new(false),
        side,
        detail: RefCell::new(None),
        play: RefCell::new(None),
        waves: RefCell::new(vec![]),
        play_buttons: RefCell::new(vec![]),
        durs: RefCell::new(HashMap::new()),
        texts: RefCell::new(HashMap::new()),
        formats: RefCell::new(HashMap::new()),
    });
    {
        let u = ui.clone();
        go_sessions.connect_clicked(move |_| u.go("sessoes"));
        let u = ui.clone();
        folder.connect_clicked(move |_| {
            let dir = expand_home(&u.core.borrow().store.prefs.default_folder);
            let _ = std::fs::create_dir_all(&dir);
            if std::process::Command::new("xdg-open").arg(&dir).spawn().is_err() {
                u.toast(tr("Could not open the folder."), None);
            }
        });
    }
    {
        let (u, pg) = (ui.clone(), page.clone());
        search.connect_changed(move |_| refresh_list(&u, &pg, false));
    }
    rescan(ui, &page, false);
    {
        let p = page.clone();
        ui.on_change(move |ui, c| match c {
            Change::Recordings => rescan(ui, &p, false),
            Change::Store => {
                refresh_header(ui, &p);
                refresh_list(ui, &p, false);
                refresh_detail(ui, &p);
            }
            Change::Layout => {
                refresh_list(ui, &p, false);
                refresh_detail(ui, &p);
                arrange(ui, &p);
            }
            _ => {}
        });
    }
    {
        let p = page.clone();
        ui.on_page(move |ui, name| {
            if name == "gravacoes" {
                rescan(ui, &p, true);
            }
        });
    }
    {
        let p = page.clone();
        ui.on_tick(move |ui| {
            let finished = p.play.borrow().as_ref().is_some_and(|pl| pl.player.as_ref().is_some_and(|x| x.finished()));
            if finished {
                if let Some(pl) = p.play.borrow_mut().as_mut() {
                    pl.player = None;
                    pl.pos = pl.dur;
                }
                p.sync_buttons();
            }
            if ui.page() != "gravacoes" {
                return;
            }
            let playing = p.play.borrow().as_ref().is_some_and(|pl| pl.player.is_some());
            let opening = p.detail.borrow().as_ref().is_some_and(|d| d.opened.elapsed().as_secs_f64() < 0.25);
            if playing {
                let path = p.play.borrow().as_ref().map(|pl| pl.path.clone());
                for (wp, w) in p.waves.borrow().iter() {
                    if Some(wp) == path.as_ref() {
                        w.queue_draw();
                    }
                }
            }
            if playing || opening {
                if let Some(d) = p.detail.borrow().as_ref() {
                    d.wave.queue_draw();
                }
            }
            if playing {
                follow_playback(&p);
            }
        });
    }
    root.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcripts_are_read_into_time_speaker_and_text() {
        let t = "[00:12] Você: Bom dia! Vamos começar?\n[16:44] Participante 2: Pode ser.\n\nplain line\n[1:02:03] no speaker here. Really: yes\n";
        let l = parse_transcript(t);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], Line { at: Some(12.0), speaker: "Você".into(), text: "Bom dia! Vamos começar?".into() });
        assert_eq!(l[1].at, Some(16.0 * 60.0 + 44.0));
        assert_eq!(l[1].speaker, "Participante 2");
        assert_eq!(l[2], Line { at: None, speaker: String::new(), text: "plain line".into() });
        assert_eq!(l[3].at, Some(3723.0));
        assert_eq!(l[3].speaker, "", "a sentence before the colon is not a speaker");
        // subtitle-style timed lines
        let w = parse_transcript("[00:00:01.500 --> 00:00:04.000]   Hello there.");
        assert_eq!(w[0].at, Some(1.5));
        assert_eq!(w[0].text, "Hello there.");
        assert_eq!(parse_stamp("ab:cd"), None);
        assert_eq!(parse_stamp("12"), None);
    }

    #[test]
    fn the_current_line_follows_the_position() {
        let l = parse_transcript("[00:10] A: one\n[00:20] B: two\n[00:30] A: three");
        assert_eq!(current_line(&l, 5.0), None);
        assert_eq!(current_line(&l, 10.0), Some(0));
        assert_eq!(current_line(&l, 25.0), Some(1));
        assert_eq!(current_line(&l, 999.0), Some(2));
    }

    #[test]
    fn subtitles_last_until_the_next_line() {
        let l = parse_transcript("[00:10] A: one\nuntimed\n[00:20] B: two\n[01:00:00] three");
        let s = to_srt(&l, Some(3602.0));
        assert_eq!(
            s,
            "1\n00:00:10,000 --> 00:00:20,000\nA: one\n\n2\n00:00:20,000 --> 01:00:00,000\nB: two\n\n3\n01:00:00,000 --> 01:00:02,000\nthree\n\n"
        );
        assert_eq!(to_srt(&parse_transcript("no times"), None), "");
        assert_eq!(srt_time(61.2345), "00:01:01,235");
    }

    #[test]
    fn names_lose_the_recorders_date() {
        assert_eq!(display_name("Reunião semanal 2026-11-19 14.02.10"), "Reunião semanal");
        assert_eq!(display_name("Kalango Cast 2026-11-18"), "Kalango Cast");
        assert_eq!(display_name("Teste de som 2026-11-18 09-12-00"), "Teste de som");
        assert_eq!(display_name("Episódio 14"), "Episódio 14");
        assert_eq!(display_name("Teste de som 21.43.53"), "Teste de som");
        assert_eq!(display_name("2026-11-19 21.51.52 Rádio"), "Rádio");
        assert_eq!(display_name("2026-11-18 09.12.00"), "2026-11-18 09.12.00");
    }

    #[test]
    fn recordings_group_by_day() {
        let midnight = 1_000_000u64;
        assert_eq!(days_ago(midnight + 10, midnight), 0);
        assert_eq!(days_ago(midnight - 1, midnight), 1);
        assert_eq!(days_ago(midnight - 86_400, midnight), 1);
        assert_eq!(days_ago(midnight - 86_401, midnight), 2);
        assert_eq!(group_of(0, (1, 1, 2026)), Group::Today);
        assert_eq!(group_of(1, (1, 1, 2026)), Group::Yesterday);
        assert_eq!(group_of(6, (1, 1, 2026)), Group::ThisWeek);
        assert_eq!(group_of(7, (9, 11, 2026)), Group::Date(9, 11, 2026));
    }

    #[test]
    fn durations_and_snippets() {
        assert_eq!(long_duration(48.0 * 60.0 + 2.0), "48 min 02 s");
        assert_eq!(long_duration(3600.0 + 12.0 * 60.0), "1 h 12 min");
        assert_eq!(long_duration(6.0), "6 s");
        assert_eq!(fmt_stamp(12.0), "00:12");
        assert_eq!(fmt_stamp(3723.0), "1:02:03");
        assert_eq!(snippet("Vamos começar pela pauta de sexta?", "pauta").as_deref(), Some("Vamos começar pela pauta de sexta?"));
        assert_eq!(snippet("Bom dia! Vamos começar pela pauta de sexta?", "pauta").as_deref(), Some("…dia! Vamos começar pela pauta de sexta?"));
        let long = format!("{}needle{}", "a".repeat(60), "b".repeat(80));
        let s = snippet(&long, "needle").unwrap();
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("needle"));
        assert_eq!(snippet("abc", "z"), None);
        assert_eq!(percent_decode("/home/a/M%C3%BAsica/x%20y.wav"), "/home/a/Música/x y.wav");
    }

    #[test]
    fn a_trashed_file_comes_back() {
        let dir = std::env::temp_dir().join(format!("kalango-grav-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let trash = dir.join("Trash");
        std::fs::create_dir_all(trash.join("info")).unwrap();
        std::fs::create_dir_all(trash.join("files")).unwrap();
        std::fs::create_dir_all(dir.join("Música")).unwrap();
        let original = dir.join("Música/a b.wav");
        std::fs::write(trash.join("files/a b.wav"), b"new").unwrap();
        std::fs::write(trash.join("files/a b.2.wav"), b"old").unwrap();
        let enc = original.to_string_lossy().replace("Música", "M%C3%BAsica").replace(' ', "%20");
        std::fs::write(trash.join("info/a b.wav.trashinfo"), format!("[Trash Info]\nPath={enc}\nDeletionDate=2026-11-19T21:51:14\n")).unwrap();
        std::fs::write(trash.join("info/a b.2.wav.trashinfo"), format!("[Trash Info]\nPath={enc}\nDeletionDate=2026-11-01T08:00:00\n")).unwrap();
        assert!(restore_from(&trash, &original));
        assert_eq!(std::fs::read(&original).unwrap(), b"new", "the newest copy comes back");
        assert!(!trash.join("info/a b.wav.trashinfo").exists());
        assert!(!restore_from(&trash, &original), "never overwrites a file that is there");
        assert!(!restore_from(&trash, &dir.join("nope.wav")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renaming_takes_the_transcript_tags_and_favourite_along() {
        let dir = std::env::temp_dir().join(format!("kalango-grav-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("Old 2026-11-19 14.02.10.wav");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(a.with_extension("txt"), "[00:01] A: hi").unwrap();
        let mut c = Core::ephemeral(vec![]);
        c.store.prefs.default_folder = dir.to_string_lossy().to_string();
        c.set_tags(&a, vec!["t".into()]);
        c.toggle_favourite(&a);
        let b = c.rename_recording(&a, "New").unwrap();
        assert_eq!(b, dir.join("New.wav"));
        assert!(b.is_file() && dir.join("New.txt").is_file() && !a.exists());
        assert_eq!(c.tags_of(&b), vec!["t".to_string()]);
        assert!(c.tags_of(&a).is_empty());
        assert!(c.is_favourite(&b) && !c.is_favourite(&a));
        std::fs::write(dir.join("Other.wav"), b"y").unwrap();
        assert!(c.rename_recording(&b, "Other").is_err(), "never overwrites");
        assert!(c.rename_recording(&b, "a/b").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
