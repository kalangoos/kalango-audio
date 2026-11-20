//! The agent's panel (design "Pedir ao agente (plano com aprovação)"): a request typed in the window's search (or
//! on first use) becomes a plan in steps — create a session from a template with some changes, the system's
//! confirmation for an app's sound, record now or at a time, transcribe at the end — with the proposed session's
//! chain in miniature. Nothing happens until the person approves; "Adjust the plan" lets them untick steps and
//! rename the session. Approving runs the steps through [`Core`](crate::core::Core) and opens the editor.
//!
//! The plan comes from [`plan`], the app's own planner: plain keyword and time rules (pt-BR and English) mapped to
//! what Kalango Audio can really do. When a request does not map, the panel says so and shows examples that work.
//!
//! TODO: hand requests the planner cannot map (summaries, free-form changes) to the KalangoOS agent through
//! kalangod (MCP), keeping this approval step; until then the panel says plainly that it is the app's planner.

use super::widgets::*;
use super::{icons, Change, Layout, Ui};
use crate::i18n::{n_, tr, trf};
use crate::model::{Automation, Template, Timer};
use gtk::prelude::*;
use gtk::{gdk, glib, Align};
use kalango_movimento as mov;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// ---------------------------------------------------------------- the planner (pure, unit-tested)

/// When the recording happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Now,
    /// once: `day_offset` days from today, at `start` minutes after midnight
    Once { day_offset: u32, start: u16 },
    /// on the days of `days` (bit 0 = Sunday), at `start`
    Repeat { days: u8, start: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// make the session from the plan's template
    Create,
    /// the system's portal asks for the app's sound (informative: nothing to run)
    AskSystem,
    /// start the session now
    RunNow,
    /// a timer starts and stops it
    Schedule,
    /// transcribe: the session's Transcribe block does it while recording (`true`) or an automation at the end
    Transcribe(bool),
    /// the stream needs its server first (informative)
    FillServer,
    StopAll,
    OpenLatestTranscript,
    TranscribeLatest,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub action: Action,
    pub title: String,
    pub detail: String,
    /// "you confirm" (the system asks) instead of "approve"
    pub system: bool,
    /// can be unticked in "Adjust the plan"
    pub optional: bool,
    pub on: bool,
}

#[derive(Clone, Debug)]
pub struct Plan {
    /// the template's name (msgid), when the plan makes a session
    pub template: Option<String>,
    pub name: String,
    /// recorder format asked for ("FLAC", "MP3"…)
    pub format: Option<String>,
    /// app the Application block listens to ("Web Browser", "Call App")
    pub app: Option<String>,
    pub when: When,
    /// how long it records (minutes); None = until stopped
    pub minutes: Option<u16>,
    pub record: bool,
    pub steps: Vec<Step>,
}

/// Lower case without accents, so "Reunião", "reuniao" and "REUNIÃO" read the same.
pub fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'ê' | 'è' | 'ë' => 'e',
            'í' | 'î' | 'ì' | 'ï' => 'i',
            'ó' | 'ô' | 'õ' | 'ò' | 'ö' => 'o',
            'ú' | 'û' | 'ù' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() || c == ':' => c,
            _ => ' ',
        })
        .collect()
}

fn words(n: &str) -> Vec<&str> {
    n.split_whitespace().collect()
}

/// Any of the keys appears as a whole word (or a phrase of whole words).
fn has(n: &str, keys: &[&str]) -> bool {
    let padded = format!(" {n} ");
    keys.iter().any(|k| padded.contains(&format!(" {k} ")))
}

/// Any word starts with one of the stems ("grav" → grave, gravar, gravação).
fn stem(n: &str, stems: &[&str]) -> bool {
    words(n).iter().any(|w| stems.iter().any(|s| w.starts_with(s)))
}

/// "20h", "8h30", "20:00", "8pm", "830pm" → (hour, minute, said am/pm).
fn hour_word(w: &str) -> Option<(u32, u32, Option<bool>)> {
    let (body, ampm) = if let Some(b) = w.strip_suffix("pm") {
        (b, Some(true))
    } else if let Some(b) = w.strip_suffix("am") {
        (b, Some(false))
    } else {
        (w, None)
    };
    let (h, m) = if let Some((a, b)) = body.split_once(':') {
        (a.parse().ok()?, if b.is_empty() { 0 } else { b.parse().ok()? })
    } else if let Some((a, b)) = body.split_once('h') {
        (a.parse().ok()?, if b.is_empty() { 0 } else { b.parse().ok()? })
    } else if ampm.is_some() {
        (body.parse().ok()?, 0)
    } else {
        return None;
    };
    if h > 23 || m > 59 || (ampm.is_some() && !(1..=12).contains(&h)) {
        return None;
    }
    Some((h, m, ampm))
}

fn to_minutes(h: u32, m: u32, pm: Option<bool>) -> u16 {
    let h = match pm {
        Some(true) if h < 12 => h + 12,
        Some(false) if h == 12 => 0,
        _ => h,
    };
    (h * 60 + m) as u16
}

/// A duration after "por"/"for"/"durante": "2 horas", "30 min", "1h30", "2h" → minutes.
fn duration_at(w: &[&str], i: usize) -> Option<(u16, usize)> {
    let a = *w.get(i)?;
    if let Some((h, m)) = a.split_once('h') {
        if let (Ok(h), Ok(m)) = (h.parse::<u16>(), if m.is_empty() { Ok(0) } else { m.parse::<u16>() }) {
            return Some((h * 60 + m, 1));
        }
    }
    if let Some(n) = a.strip_suffix("min").and_then(|x| x.parse::<u16>().ok()) {
        return Some((n, 1));
    }
    let n: u16 = match a {
        "uma" | "um" | "one" | "an" | "a" => 1,
        "meia" | "half" => return if matches!(w.get(i + 1), Some(&"hora") | Some(&"hour")) { Some((30, 2)) } else { None },
        x => x.parse().ok()?,
    };
    let unit = *w.get(i + 1)?;
    if unit.starts_with("hora") || unit.starts_with("hour") || unit == "h" || unit == "hrs" {
        Some((n * 60, 2))
    } else if unit.starts_with("min") {
        Some((n, 2))
    } else {
        None
    }
}

const WEEKDAYS: [(&[&str], u8); 7] = [
    (&["domingo", "domingos", "sunday", "sundays"], 0),
    (&["segunda", "segundas", "monday", "mondays"], 1),
    (&["terca", "tercas", "tuesday", "tuesdays"], 2),
    (&["quarta", "quartas", "wednesday", "wednesdays"], 3),
    (&["quinta", "quintas", "thursday", "thursdays"], 4),
    (&["sexta", "sextas", "friday", "fridays"], 5),
    (&["sabado", "sabados", "saturday", "saturdays"], 6),
];

/// The time asked for, and how long: (when, minutes). `now_min` and `weekday` (0 = Sunday) are "now".
pub fn parse_when(n: &str, now_min: u32, weekday: u32) -> (When, Option<u16>) {
    let w = words(n);
    let mut used = vec![false; w.len()];
    // how long first, so "por 2h" is not read as a time
    let mut minutes = None;
    for i in 0..w.len() {
        if matches!(w[i], "por" | "for" | "durante" | "during") {
            if let Some((m, len)) = duration_at(&w, i + 1) {
                minutes = Some(m);
                for u in used.iter_mut().skip(i).take(len + 1) {
                    *u = true;
                }
            }
        }
    }
    // the time of day
    let mut start: Option<u16> = None;
    for i in 0..w.len() {
        if used[i] || start.is_some() {
            continue;
        }
        if let Some((h, m, ampm)) = hour_word(w[i]) {
            let pm = ampm.or_else(|| part_of_day(&w, i + 1));
            start = Some(to_minutes(h, m, pm));
        } else if matches!(w[i], "as" | "at" | "a") {
            if let Some(h) = w.get(i + 1).and_then(|x| x.parse::<u32>().ok()).filter(|h| *h <= 23) {
                let (ampm, skip) = match w.get(i + 2) {
                    Some(&"pm") => (Some(true), 1),
                    Some(&"am") => (Some(false), 1),
                    _ => (part_of_day(&w, i + 2), 0),
                };
                let _ = skip;
                start = Some(to_minutes(h, 0, ampm));
            }
        }
    }
    if has(n, &["meio dia", "meio-dia", "noon", "midday"]) && start.is_none() {
        start = Some(12 * 60);
    }
    if has(n, &["meia noite", "midnight"]) && start.is_none() {
        start = Some(0);
    }
    // the days
    let mut mask = 0u8;
    for (names, d) in WEEKDAYS {
        if has(n, names) {
            mask |= 1 << d;
        }
    }
    let every = has(n, &["todo", "toda", "todos", "todas", "every", "each"]) || stem(n, &["diariamente", "daily"]);
    let all_days = has(n, &["todo dia", "todos os dias", "toda dia", "every day", "everyday", "daily", "diariamente", "each day"]);
    let workdays = has(n, &["dias de semana", "dia util", "dias uteis", "weekdays", "every weekday"]);
    let Some(start) = start else { return (When::Now, minutes) };
    if workdays {
        return (When::Repeat { days: 0b0111110, start }, minutes);
    }
    if all_days {
        return (When::Repeat { days: 0b1111111, start }, minutes);
    }
    if mask != 0 {
        if every || words(n).iter().any(|x| WEEKDAYS.iter().any(|(ns, _)| ns.iter().any(|y| y.ends_with('s') && y == x))) {
            return (When::Repeat { days: mask, start }, minutes);
        }
        // the next one of that day
        let d = mask.trailing_zeros();
        let mut off = (d + 7 - weekday) % 7;
        if off == 0 && start as u32 <= now_min {
            off = 7;
        }
        return (When::Once { day_offset: off, start }, minutes);
    }
    let off = if has(n, &["depois de amanha", "day after tomorrow"]) {
        2
    } else if has(n, &["amanha", "tomorrow"]) {
        1
    } else if start as u32 <= now_min {
        // a time already gone today is tomorrow's
        1
    } else {
        0
    };
    (When::Once { day_offset: off, start }, minutes)
}

/// "da noite", "da tarde", "da manhã", "in the evening" after a time.
fn part_of_day(w: &[&str], i: usize) -> Option<bool> {
    let a = w.get(i).copied().unwrap_or("");
    let b = w.get(i + 1).copied().unwrap_or("");
    let c = w.get(i + 2).copied().unwrap_or("");
    match (a, b, c) {
        ("da" | "de", "noite" | "tarde", _) | ("in", "the", "evening" | "afternoon") | ("at", "night", _) | ("tonight", _, _) => Some(true),
        ("da" | "de", "manha" | "madrugada", _) | ("in", "the", "morning") => Some(false),
        _ => None,
    }
}

fn hm(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// The days of a repeat in words: "every day", "weekdays", "Mon, Wed".
fn days_text(mask: u8) -> String {
    match mask {
        0b1111111 => tr("every day").to_string(),
        0b0111110 => tr("on weekdays").to_string(),
        _ => {
            let names = [n_("Sun"), n_("Mon"), n_("Tue"), n_("Wed"), n_("Thu"), n_("Fri"), n_("Sat")];
            let v: Vec<&str> = (0..7).filter(|d| mask & (1 << d) != 0).map(|d| tr(names[d])).collect();
            trf("every {days}", &[("days", v.join(", "))])
        }
    }
}

/// When, in words: "now", "today at 20:00", "tomorrow at 08:00", "in 3 days at 09:00", "every day at 20:00".
pub fn when_text(w: When) -> String {
    match w {
        When::Now => tr("now").to_string(),
        When::Once { day_offset: 0, start } => trf("today at {time}", &[("time", hm(start))]),
        When::Once { day_offset: 1, start } => trf("tomorrow at {time}", &[("time", hm(start))]),
        When::Once { day_offset, start } => trf("in {n} days at {time}", &[("n", day_offset.to_string()), ("time", hm(start))]),
        When::Repeat { days, start } => trf("{days} at {time}", &[("days", days_text(days)), ("time", hm(start))]),
    }
}

/// The text between quotes in a request (“…”, "…" or «…»), if any.
pub fn quoted(s: &str) -> Option<String> {
    for (a, b) in [('“', '”'), ('"', '"'), ('«', '»')] {
        if let Some(i) = s.find(a) {
            let rest = &s[i + a.len_utf8()..];
            if let Some(j) = rest.find(b) {
                let q = rest[..j].trim();
                if !q.is_empty() {
                    return Some(q.to_string());
                }
            }
        }
    }
    None
}

/// Which template a request is about (in order of how specific the words are).
fn pick_template(n: &str) -> Option<&'static str> {
    let rules: [(&[&str], &str); 10] = [
        (&["podcast"], "Remote podcast"),
        (&["radio", "icecast"], "Online radio"),
        (&["live", "rtmp", "ao vivo", "youtube live", "twitch", "transmitir", "transmita", "transmissao", "stream", "streaming"], "RTMP live"),
        (&["aula", "aulas", "palestra", "class", "lecture", "seminario", "seminar"], "Class or lecture"),
        (&["reuniao", "reunioes", "meeting", "meetings", "chamada", "call", "meet", "zoom", "teams", "skype", "videoconferencia"], "Online meeting"),
        (&["mais alto", "louder", "aumentar o volume", "aumente o volume", "volume mais alto", "boost"], "Listen louder"),
        (&["monitorar", "monitor", "retorno", "ouvir minha voz", "hear myself", "ouvir a minha voz"], "Monitor microphone"),
        (&["microfone", "microphone", "mic", "voz", "voice", "narracao", "narration", "locucao", "voice over", "voiceover"], "Clean narration"),
        (&["navegador", "browser", "aba", "tab", "aplicativo", "app", "programa", "spotify", "firefox", "chrome", "jogo", "game"], "Record an app"),
        (&["computador", "computer", "sistema", "system", "tudo", "everything", "pc"], "Record the computer"),
    ];
    rules.iter().find(|(k, _)| has(n, k)).map(|(_, t)| *t)
}

/// Turns a request into a plan the app can carry out, or None when it does not map to anything it can do.
/// `now_min` (minutes after midnight) and `weekday` (0 = Sunday) are "now", for times like "às 20h".
pub fn plan(text: &str, templates: &[Template], now_min: u32, weekday: u32) -> Option<Plan> {
    let n = normalize(text);
    if n.trim().is_empty() {
        return None;
    }
    // the verbs only: "a última gravação" / "the last recording" is not a request to record
    let record = has(&n, &["grave", "gravar", "grava", "gravem", "gravando", "record", "capture", "captura", "capturar", "capturem", "tape"]);
    let transcribe = stem(&n, &["transcrev", "transcri", "texto", "text"]) && !has(&n, &["texto do", "text of"]);
    let blank = |action, title: String, detail: String| Step { action, title, detail, system: false, optional: false, on: true };
    let empty = |steps| Plan { template: None, name: String::new(), format: None, app: None, when: When::Now, minutes: None, record: false, steps };

    // stop everything
    if has(&n, &["pare", "parar", "stop", "encerre", "encerrar", "termine"]) && !record {
        return Some(empty(vec![blank(Action::StopAll, tr("Stop every running session").to_string(), tr("recordings are closed properly first").to_string())]));
    }
    // summaries: transcribe first when there is no transcript, then the local model sums it up
    if stem(&n, &["resum", "summar"]) {
        let mut pl = empty(vec![blank(
            Action::OpenLatestTranscript,
            tr("Summarize the recording").to_string(),
            tr("transcribed first with the voice model if it has no transcript; the local model writes the summary, saved beside the audio").to_string(),
        )]);
        // (the recording, when the request names it between quotes)
        pl.name = quoted(text).unwrap_or_default();
        return Some(pl);
    }
    let template = pick_template(&n);
    // "transcreva a última gravação": transcribe what is already recorded
    if transcribe && !record && (template.is_none() || has(&n, &["ultima", "last", "latest", "recente", "recent"])) && stem(&n, &["grava", "record", "audio", "arquivo", "file"]) {
        return Some(empty(vec![blank(
            Action::TranscribeLatest,
            tr("Transcribe the latest recording now").to_string(),
            tr("KalangoOS’s voice model turns it into text, on this computer; the text goes next to the audio").to_string(),
        )]));
    }
    let template = match template {
        Some(t) => t,
        None if record => "Record the computer",
        None => return None,
    };
    let tpl = templates.iter().find(|t| t.name == template)?;
    let kinds: Vec<&str> = tpl.blocks.iter().map(|b| b.kind.as_str()).collect();
    let records = kinds.contains(&"recorder");
    let streams = kinds.iter().any(|k| *k == "radio_stream" || *k == "rtmp_stream");
    let (when, minutes) = parse_when(&n, now_min, weekday);
    let format = ["flac", "mp3", "wav", "aac", "aiff", "alac"].iter().find(|f| has(&n, &[f])).map(|f| f.to_uppercase()).filter(|_| records);
    let call = has(&n, &["zoom", "teams", "skype", "chamada de video", "video call"]);
    let browser = has(&n, &["navegador", "browser", "aba", "tab", "meet", "firefox", "chrome"]);
    let app = if !kinds.contains(&"app_source") {
        None
    } else if call {
        Some("Call App".to_string())
    } else if browser {
        Some("Web Browser".to_string())
    } else {
        None
    };
    let name = match template {
        "Online meeting" if call => tr("Call meeting").to_string(),
        "Online meeting" => tr("Browser meeting").to_string(),
        "Record an app" if browser => tr("Browser sound").to_string(),
        t => tr(t).to_string(),
    };

    let mut steps = vec![];
    let mut changes: Vec<String> = vec![];
    if let Some(f) = &format {
        changes.push(trf("recorded as {format}", &[("format", f.clone())]));
    }
    if let (Some(m), true) = (minutes, records) {
        changes.push(trf("stops after {n} min", &[("n", m.to_string())]));
    }
    let mut detail = trf("from the template {template}", &[("template", tr(template).to_string())]);
    if !changes.is_empty() {
        detail = format!("{detail} · {}", changes.join(" · "));
    }
    steps.push(Step { action: Action::Create, title: trf("Create the session “{name}”", &[("name", name.clone())]), detail, system: false, optional: false, on: true });
    if kinds.contains(&"app_source") {
        let (title, d) = if app.as_deref() == Some("Web Browser") || (template == "Online meeting" && !call) {
            (tr("Take only the sound of the browser tab"), tr("the system’s portal asks for your permission"))
        } else {
            (tr("Take only the app’s sound"), tr("the system’s portal asks for your permission"))
        };
        steps.push(Step { action: Action::AskSystem, title: title.to_string(), detail: d.to_string(), system: true, optional: false, on: true });
    }
    let doing = if records { tr("Record") } else if streams { tr("Go live") } else { tr("Start") };
    match when {
        When::Now if streams => steps.push(Step {
            action: Action::FillServer,
            title: tr("Fill in the server, then go live").to_string(),
            detail: tr("the editor opens on the session; the stream needs its address and key before it starts").to_string(),
            system: false,
            optional: false,
            on: true,
        }),
        When::Now => steps.push(Step {
            action: Action::RunNow,
            title: if records { tr("Record now, until you stop it") } else { tr("Start now") }.to_string(),
            detail: match (records, minutes) {
                (true, Some(m)) => trf("it stops by itself after {n} min, or when you press Stop (Ctrl+R)", &[("n", m.to_string())]),
                (true, None) => tr("it stops when you press Stop (Ctrl+R)").to_string(),
                _ => tr("you hear the result right away; Stop (Ctrl+R) ends it").to_string(),
            },
            system: false,
            optional: true,
            on: true,
        }),
        w => steps.push(Step {
            action: Action::Schedule,
            title: format!("{doing} {}", when_text(w)),
            detail: trf("a schedule starts and stops the session ({n} min); the computer must be on", &[("n", minutes.unwrap_or(60).to_string())]),
            system: false,
            optional: true,
            on: true,
        }),
    }
    if transcribe && records {
        let block = kinds.contains(&"transcribe");
        steps.push(Step {
            action: Action::Transcribe(block),
            title: if block { tr("Transcribe while recording") } else { tr("Transcribe at the end") }.to_string(),
            detail: if block {
                tr("the Transcribe block writes the text next to the audio; nothing leaves the computer").to_string()
            } else {
                tr("an automation runs the built-in “Transcribe when done” script with the voice model when the file is finished").to_string()
            },
            system: false,
            optional: true,
            on: true,
        });
    }
    Some(Plan { template: Some(template.to_string()), name, format, app, when, minutes, record: records && (record || template == "Online meeting" || template == "Class or lecture" || template == "Remote podcast"), steps })
}

// ---------------------------------------------------------------- carrying it out

/// The built-in script that transcribes the recording an event brings.
fn transcribe_script() -> Option<&'static str> {
    crate::script::BUILTIN.iter().find(|(_, t)| t.contains(".transcribe(")).map(|(n, _)| *n)
}

fn date_in(days: u32) -> u32 {
    glib::DateTime::now_local()
        .ok()
        .and_then(|d| d.add_days(days as i32).ok())
        .map(|d| {
            let (y, m, dd) = d.ymd();
            (y as u32) * 10000 + (m as u32) * 100 + dd as u32
        })
        .unwrap_or(0)
}

/// Runs the approved steps; returns the session to open (if one was made) and what to tell the person.
fn execute(ui: &Rc<Ui>, p: &Plan) -> (Option<usize>, String) {
    let on = |a: &dyn Fn(&Action) -> bool| p.steps.iter().any(|s| s.on && a(&s.action));
    if on(&|a| *a == Action::StopAll) {
        let n = ui.core.borrow().running.len();
        ui.core.borrow_mut().stop_all();
        ui.changed(Change::Running);
        return (None, trf("{n} session(s) stopped.", &[("n", n.to_string())]));
    }
    if on(&|a| *a == Action::OpenLatestTranscript) {
        // the recording named in the request (“summarize the recording “X””), else the latest one
        let target = {
            let mut c = ui.core.borrow_mut();
            if c.rec_scanned.is_none() {
                c.scan_recordings();
            }
            let q = p.name.trim().to_lowercase();
            let named = (!q.is_empty()).then(|| c.recs.iter().find(|r| r.name.to_lowercase().contains(&q)).map(|r| r.path.clone())).flatten();
            named.or_else(|| c.recs.iter().max_by_key(|r| r.modified).map(|r| r.path.clone()))
        };
        return match target {
            Some(path) => {
                super::gravacoes::summarize(ui, &path);
                (None, trf("Summarizing {file}; the progress shows in Recordings.", &[("file", path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default())]))
            }
            None => (None, tr("There are no recordings yet.").to_string()),
        };
    }
    if on(&|a| *a == Action::TranscribeLatest) {
        let Some(script) = transcribe_script() else { return (None, tr("The transcription script is missing.").to_string()) };
        {
            let mut c = ui.core.borrow_mut();
            if c.rec_scanned.is_none() {
                c.scan_recordings();
            }
            c.run_script_now(script);
        }
        ui.changed(Change::Recordings);
        return (None, tr("Transcribing the latest recording; the text appears next to it when done.").to_string());
    }
    let Some(tname) = &p.template else { return (None, String::new()) };
    let idx = {
        let mut c = ui.core.borrow_mut();
        let Some(t) = c.templates.iter().position(|t| t.name == *tname) else { return (None, String::new()) };
        let i = c.new_with_prefs(t);
        let taken = c.sessions.iter().enumerate().any(|(k, s)| k != i && s.name == p.name);
        // the automation's script: the built-in one (KalangoOS's voice model, the system's language)
        let script = transcribe_script().map(str::to_string);
        let s = &mut c.sessions[i];
        if !p.name.trim().is_empty() && !taken {
            s.name = p.name.trim().to_string();
        }
        for b in s.blocks.iter_mut() {
            match b.kind.as_str() {
                "recorder" => {
                    if let Some(f) = &p.format {
                        b.p.set_text("format", f);
                    }
                    if let Some(m) = p.minutes {
                        b.p.set("stop_on", 1.0);
                        b.p.set("stop_size", m as f64);
                        b.p.set_text("stop_unit", "time*60");
                    }
                }
                "app_source" => {
                    if let Some(a) = &p.app {
                        b.p.set_text("app", a);
                    }
                }
                _ => {}
            }
        }
        // transcription at the end: the built-in script, with the model from Settings when there is one
        if p.steps.iter().any(|s| s.on && s.action == Action::Transcribe(false)) {
            s.automations.push(Automation { event: "recording_finished".into(), trigger: 0, minutes: 0, script: String::new(), on: true });
        }
        for a in s.automations.iter_mut().filter(|a| a.script.is_empty()) {
            a.script = script.clone().unwrap_or_default();
        }
        s.automations.retain(|a| !a.script.is_empty());
        if p.steps.iter().any(|s| s.on && s.action == Action::Schedule) {
            let minutes = p.minutes.unwrap_or(60);
            match p.when {
                When::Once { day_offset, start } => s.timers.push(Timer { days: 0, start, minutes, on: true, mode: 0, date: date_in(day_offset), quit_sources: false }),
                When::Repeat { days, start } => s.timers.push(Timer { days, start, minutes, on: true, mode: 1, date: 0, quit_sources: false }),
                When::Now => {}
            }
        }
        i
    };
    ui.changed(Change::Sessions);
    let run = p.steps.iter().any(|s| s.on && s.action == Action::RunNow);
    if run {
        ui.toggle(idx);
    }
    let name = ui.core.borrow().sessions.get(idx).map(|s| s.name.clone()).unwrap_or_default();
    let msg = if run {
        trf("“{name}” created and started.", &[("name", name)])
    } else if p.steps.iter().any(|s| s.on && s.action == Action::Schedule) {
        trf("“{name}” created and scheduled {when}.", &[("name", name), ("when", when_text(p.when))])
    } else {
        trf("“{name}” created.", &[("name", name)])
    };
    (Some(idx), msg)
}

// ---------------------------------------------------------------- the panel

struct Panel {
    id: u32,
    content: gtk::Box,
    title: gtk::Label,
    approve: gtk::Button,
    adjust: gtk::Button,
    request: RefCell<String>,
    plan: RefCell<Option<Plan>>,
    adjusting: Cell<bool>,
    close: Rc<dyn Fn()>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Panel>>> = const { RefCell::new(None) };
    static NEXT_ID: Cell<u32> = const { Cell::new(0) };
}

const EXAMPLES: [&str; 5] = [
    n_("record the meeting in the browser and transcribe it"),
    n_("record tomorrow’s class at 8 and transcribe"),
    n_("listen louder"),
    n_("record my microphone in FLAC for 30 minutes"),
    n_("stop everything"),
];

fn examples(ui: &Rc<Ui>, panel: &Rc<Panel>, b: &gtk::Box) {
    let cap = caps(tr("Try"));
    cap.set_margin_top(8);
    b.append(&cap);
    for e in EXAMPLES {
        let x = gtk::Button::with_label(tr(e));
        x.add_css_class("ka-agente-exemplo");
        x.set_halign(Align::Start);
        let (u, p) = (ui.clone(), panel.clone());
        x.connect_clicked(move |_| {
            *p.request.borrow_mut() = tr(e).to_string();
            render(&u, &p);
        });
        b.append(&x);
    }
}

fn tag(step: &Step) -> gtk::Label {
    if step.system {
        chip(tr("you confirm"), "gold")
    } else {
        chip(tr("approve"), "accent")
    }
}

fn render(ui: &Rc<Ui>, panel: &Rc<Panel>) {
    clear(&panel.content);
    let text = panel.request.borrow().clone();
    let c = &panel.content;
    if text.trim().is_empty() {
        panel.title.set_text(tr("Agent"));
        let e = entry(tr("What should I do? E.g. “record the meeting and transcribe”"), Some("spark"));
        e.add_css_class("ka-agente-campo");
        let (u, p) = (ui.clone(), panel.clone());
        e.connect_activate(move |e| {
            let t = e.text().to_string();
            if t.trim().is_empty() {
                return;
            }
            *p.request.borrow_mut() = t;
            render(&u, &p);
        });
        c.append(&e);
        glib::idle_add_local_once(move || {
            e.grab_focus();
        });
        examples(ui, panel, c);
        *panel.plan.borrow_mut() = None;
        panel.approve.set_visible(false);
        panel.adjust.set_visible(false);
        return;
    }
    // the request, as a bubble on the right
    let bubble = label(&text, "ka-agente-pedido");
    bubble.set_wrap(true);
    bubble.set_max_width_chars(40);
    bubble.set_halign(Align::End);
    c.append(&bubble);

    let (now_wd, now_min) = crate::engine::local_now();
    let fresh = panel.plan.borrow().is_none();
    if fresh {
        let p = plan(&text, &ui.core.borrow().templates, now_min, now_wd);
        *panel.plan.borrow_mut() = p;
    }
    let Some(p) = panel.plan.borrow().clone() else {
        panel.title.set_text(tr("Agent"));
        c.append(&para(tr("I can’t do that here yet."), "ka-titulo3"));
        c.append(&para(
            tr("This is Kalango Audio’s own planner: it understands requests to record, schedule, transcribe, listen louder or go live, and builds them from the app’s templates."),
            "ka-texto2",
        ));
        examples(ui, panel, c);
        panel.approve.set_visible(false);
        panel.adjust.set_visible(false);
        return;
    };
    panel.approve.set_visible(true);
    panel.adjust.set_visible(p.template.is_some());
    panel.title.set_text(&if p.template.is_some() { trf("Agent · plan for “{name}”", &[("name", p.name.clone())]) } else { tr("Agent · plan").to_string() });
    let intro = para(tr("Here’s how I’ll do it. Nothing happens without your approval, and you can stop at any time."), "ka-texto2");
    c.append(&intro);

    // the session it will make
    if let Some(tname) = &p.template {
        let card = vbox(10, "ka-cartao2 ka-agente-sessao");
        let top = hbox(8, "");
        if panel.adjusting.get() {
            top.append(&label(tr("New session"), "ka-titulo3"));
            let e = entry(tr("Session name"), None);
            e.set_text(&p.name);
            e.set_hexpand(true);
            let pp = panel.clone();
            e.connect_changed(move |e| {
                if let Some(pl) = pp.plan.borrow_mut().as_mut() {
                    pl.name = e.text().to_string();
                }
            });
            top.append(&e);
        } else {
            top.append(&line(&trf("New session “{name}”", &[("name", p.name.clone())]), "ka-titulo3"));
        }
        card.append(&top);
        let kinds: Vec<String> = {
            let cc = ui.core.borrow();
            cc.templates.iter().find(|t| t.name == *tname).map(|t| ordered_kinds(&t.instantiate(), 8)).unwrap_or_default()
        };
        card.append(&chain(&kinds, 32, 14));
        let words: Vec<String> = kinds
            .iter()
            .map(|k| {
                let t = crate::model::kind_info(k).map(|x| tr(x.title).to_string()).unwrap_or_default();
                match (k.as_str(), &p.format) {
                    ("recorder", Some(f)) => format!("{t} ({f})"),
                    ("app_source", _) => p.app.as_deref().map(crate::audio::app_label).unwrap_or(t),
                    _ => t,
                }
            })
            .collect();
        card.append(&para(&words.join(" → "), "ka-fraco2"));
        c.append(&card);
    }

    // the steps
    for (i, s) in p.steps.iter().enumerate() {
        let r = hbox(12, "ka-cartao2 ka-agente-passo");
        if !s.on {
            r.add_css_class("desligado");
        }
        if panel.adjusting.get() && s.optional {
            let cb = gtk::CheckButton::new();
            cb.set_active(s.on);
            cb.set_valign(Align::Center);
            let (u, pp) = (ui.clone(), panel.clone());
            cb.connect_toggled(move |cb| {
                if let Some(pl) = pp.plan.borrow_mut().as_mut() {
                    if let Some(st) = pl.steps.get_mut(i) {
                        st.on = cb.is_active();
                    }
                }
                let (u, pp) = (u.clone(), pp.clone());
                glib::idle_add_local_once(move || render(&u, &pp));
            });
            r.append(&cb);
        } else {
            let n = label(&(i + 1).to_string(), "ka-agente-numero");
            n.set_valign(Align::Center);
            r.append(&n);
        }
        let t = vbox(3, "");
        t.set_hexpand(true);
        t.append(&para(&s.title, "ka-texto"));
        let mut d = s.detail.clone();
        if s.action == Action::StopAll {
            let n = ui.core.borrow().running.len();
            d = format!("{d} · {}", trf("{n} running now", &[("n", n.to_string())]));
        }
        if matches!(s.action, Action::Transcribe(_) | Action::TranscribeLatest) && crate::voz::status() != crate::voz::Status::Ready {
            d = format!("{d} · {}", tr("The voice model is not installed — get it in the App Center"));
        }
        t.append(&para(&d, "ka-fraco"));
        r.append(&t);
        let tg = tag(s);
        tg.set_valign(Align::Center);
        r.append(&tg);
        c.append(&r);
    }

    // the limits
    let lim = hbox(12, "ka-agente-limites");
    let sh = icons::icon("escudo", "accent", 16);
    sh.set_valign(Align::Start);
    lim.append(&sh);
    lim.append(&para(
        tr("Limits: only Kalango Audio’s own tools (create a session, schedule, start, stop, transcribe). Nothing is deleted, the session stays in your list, and Stop (Ctrl+R) ends it at any time. Esc closes this panel."),
        "ka-texto2",
    ));
    c.append(&lim);

    // the main button says what will happen
    let run = p.steps.iter().any(|s| s.on && s.action == Action::RunNow);
    let sched = p.steps.iter().any(|s| s.on && s.action == Action::Schedule);
    let label_text = if p.steps.iter().any(|s| s.action == Action::StopAll) {
        tr("Approve and stop")
    } else if run && p.record {
        tr("Approve and record")
    } else if run {
        tr("Approve and start")
    } else if sched {
        tr("Approve and schedule")
    } else {
        tr("Approve")
    };
    if let Some(l) = panel.approve.child().and_then(|r| r.last_child()).and_downcast::<gtk::Label>() {
        l.set_text(label_text);
    }
    if let Some(l) = panel.adjust.child().and_downcast::<gtk::Label>() {
        l.set_text(if panel.adjusting.get() { tr("Done adjusting") } else { tr("Adjust the plan") });
    }
}

/// Opens the agent's panel with a request (an empty one shows a field and examples).
pub fn ask(ui: &Rc<Ui>, text: &str) {
    if let Some(p) = OPEN.with(|o| o.borrow_mut().take()) {
        (p.close)();
    }
    let veil = gtk::Box::new(gtk::Orientation::Vertical, 0);
    veil.add_css_class("ka-veu");
    veil.set_hexpand(true);
    veil.set_vexpand(true);
    ui.overlay.add_overlay(&veil);

    let shell = vbox(0, "ka-agente");
    let width = if ui.layout() == Layout::Narrow { (ui.window.width() - 32).clamp(300, 520) } else { 520 };
    shell.set_size_request(width, -1);
    // header
    let head = hbox(10, "ka-agente-topo");
    head.append(&icons::icon("spark", "accent", 16));
    let title = line(tr("Agent"), "ka-titulo3");
    head.append(&title);
    let wait = chip(tr("waiting for you"), "gold");
    head.append(&wait);
    let close_b = icon_button("fechar", "muted", 14, tr("Close"));
    head.append(&close_b);
    shell.append(&head);
    let content = vbox(12, "ka-agente-corpo");
    let sc = scroller(&content);
    shell.append(&sc);
    // footer
    let foot = hbox(8, "ka-agente-rodape");
    let cancel = button(tr("Cancel"), "fantasma", None);
    let adjust = gtk::Button::with_label(tr("Adjust the plan"));
    adjust.add_css_class("ka-botao");
    adjust.add_css_class("ka-fantasma");
    adjust.set_valign(Align::Center);
    foot.append(&cancel);
    foot.append(&adjust);
    foot.append(&spacer());
    let approve = button(tr("Approve and record"), "primario", Some("check"));
    foot.append(&approve);
    shell.append(&foot);

    let anim = animated(&shell);
    anim.set_halign(Align::End);
    anim.set_valign(Align::Fill);
    anim.set_margin_top(12);
    anim.set_margin_bottom(12);
    anim.set_margin_end(12);
    ui.overlay.add_overlay(&anim);
    anim.entrar((48.0, 0.0), 1.0);
    veil.set_opacity(0.0);
    fade(&veil, 1.0);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let done = Rc::new(Cell::new(false));
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let close: Rc<dyn Fn()> = {
        let (ov, veil, anim, win, keys, done) = (ui.overlay.clone(), veil.clone(), anim.clone(), ui.window.clone(), keys.clone(), done.clone());
        Rc::new(move || {
            if done.replace(true) {
                return;
            }
            win.remove_controller(&keys);
            OPEN.with(|o| {
                let mut o = o.borrow_mut();
                if o.as_ref().map(|p| p.id == id).unwrap_or(false) {
                    *o = None;
                }
            });
            let (ov2, a2) = (ov.clone(), anim.clone());
            anim.sair((48.0, 0.0), 1.0, move || ov2.remove_overlay(&a2));
            let (ov3, v3) = (ov.clone(), veil.clone());
            fade(&veil, 0.0);
            glib::timeout_add_local_once(std::time::Duration::from_millis(260), move || ov3.remove_overlay(&v3));
        })
    };
    let panel = Rc::new(Panel {
        id,
        content,
        title,
        approve: approve.clone(),
        adjust: adjust.clone(),
        request: RefCell::new(text.to_string()),
        plan: RefCell::new(None),
        adjusting: Cell::new(false),
        close: close.clone(),
    });
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
    let click = gtk::GestureClick::new();
    {
        let c = close.clone();
        click.connect_pressed(move |_, _, _, _| c());
    }
    veil.add_controller(click);
    {
        let c = close.clone();
        close_b.connect_clicked(move |_| c());
        let c = close.clone();
        cancel.connect_clicked(move |_| c());
    }
    {
        let (u, p) = (ui.clone(), panel.clone());
        adjust.connect_clicked(move |_| {
            p.adjusting.set(!p.adjusting.get());
            render(&u, &p);
        });
    }
    {
        let (u, p) = (ui.clone(), panel.clone());
        approve.connect_clicked(move |_| {
            let Some(plan) = p.plan.borrow().clone() else { return };
            (p.close)();
            let (idx, msg) = execute(&u, &plan);
            if let Some(i) = idx {
                u.open_session(i);
            }
            if !msg.is_empty() {
                u.toast(&msg, None);
            }
        });
    }
    render(ui, &panel);
    OPEN.with(|o| *o.borrow_mut() = Some(panel));
}

/// The veil's opacity follows the suave spring.
fn fade(w: &gtk::Box, to: f64) {
    let m = RefCell::new({
        let mut m = mov::Mola::new(w.opacity());
        m.ir(to, mov::SUAVE);
        m
    });
    w.add_tick_callback(move |w, _| {
        let mut s = m.borrow_mut();
        s.passo(1.0 / 60.0);
        w.set_opacity(s.x.clamp(0.0, 1.0));
        if s.parada() {
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::templates;

    fn p(text: &str) -> Option<Plan> {
        // "now" is Wednesday 10:00
        plan(text, &templates(), 10 * 60, 3)
    }

    fn actions(pl: &Plan) -> Vec<Action> {
        pl.steps.iter().map(|s| s.action.clone()).collect()
    }

    #[test]
    fn meeting_in_the_browser_pt_and_en() {
        for text in ["grave a reunião do navegador e transcreva", "record the meeting in the browser and transcribe it", "Grave a REUNIAO do Meet e transcreva"] {
            let pl = p(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(pl.template.as_deref(), Some("Online meeting"), "{text}");
            assert_eq!(pl.name, "Browser meeting");
            assert_eq!(pl.app.as_deref(), Some("Web Browser"));
            assert_eq!(pl.when, When::Now);
            assert!(pl.record);
            assert_eq!(actions(&pl), vec![Action::Create, Action::AskSystem, Action::RunNow, Action::Transcribe(true)], "{text}");
            assert!(pl.steps[1].system, "the portal step is the system's to confirm");
        }
    }

    #[test]
    fn calls_listen_to_the_call_app() {
        let pl = p("grave a chamada do zoom").unwrap();
        assert_eq!(pl.template.as_deref(), Some("Online meeting"));
        assert_eq!(pl.app.as_deref(), Some("Call App"));
        assert_eq!(pl.name, "Call meeting");
    }

    #[test]
    fn times_and_days() {
        let pl = p("grave a aula de amanhã às 8h e transcreva").unwrap();
        assert_eq!(pl.template.as_deref(), Some("Class or lecture"));
        assert_eq!(pl.when, When::Once { day_offset: 1, start: 8 * 60 });
        assert_eq!(actions(&pl), vec![Action::Create, Action::Schedule, Action::Transcribe(true)]);
        assert_eq!(p("record the class at 20h").unwrap().when, When::Once { day_offset: 0, start: 20 * 60 });
        assert_eq!(p("grave às 20h30").unwrap().when, When::Once { day_offset: 0, start: 20 * 60 + 30 });
        assert_eq!(p("record at 7:45pm").unwrap().when, When::Once { day_offset: 0, start: 19 * 60 + 45 });
        assert_eq!(p("record tomorrow at 8pm").unwrap().when, When::Once { day_offset: 1, start: 20 * 60 });
        assert_eq!(p("grave amanhã às 8 da noite").unwrap().when, When::Once { day_offset: 1, start: 20 * 60 });
        assert_eq!(p("grave às 9 da manhã").unwrap().when, When::Once { day_offset: 1, start: 9 * 60 }, "9:00 is gone today (now is 10:00)");
        assert_eq!(p("grave depois de amanhã às 14:00").unwrap().when, When::Once { day_offset: 2, start: 14 * 60 });
        assert_eq!(p("grave a reunião na sexta às 14h").unwrap().when, When::Once { day_offset: 2, start: 14 * 60 });
        assert_eq!(p("record the meeting on monday at 9am").unwrap().when, When::Once { day_offset: 5, start: 9 * 60 });
        assert_eq!(p("grave a rádio ao meio-dia").unwrap().when, When::Once { day_offset: 0, start: 12 * 60 });
    }

    #[test]
    fn repeats_and_durations() {
        let pl = p("transmita a rádio todo dia às 20h por 2 horas").unwrap();
        assert_eq!(pl.template.as_deref(), Some("Online radio"));
        assert_eq!(pl.when, When::Repeat { days: 0b1111111, start: 20 * 60 });
        assert_eq!(pl.minutes, Some(120));
        assert_eq!(p("RTMP live every monday at 21:00").unwrap().when, When::Repeat { days: 0b10, start: 21 * 60 });
        assert_eq!(p("grave a reunião todas as segundas e quartas às 9h").unwrap().when, When::Repeat { days: 0b1010, start: 9 * 60 });
        assert_eq!(p("grave a aula nos dias de semana às 8h").unwrap().when, When::Repeat { days: 0b0111110, start: 8 * 60 });
        let pl = p("record my microphone for 30 minutes").unwrap();
        assert_eq!((pl.when, pl.minutes), (When::Now, Some(30)));
        assert_eq!(p("grave o microfone por 1h30").unwrap().minutes, Some(90));
        assert_eq!(p("grave o microfone por meia hora").unwrap().minutes, Some(30));
        assert!(p("record my microphone for 30 minutes").unwrap().steps[0].detail.contains("stops after 30 min"));
    }

    #[test]
    fn templates_by_words() {
        let t = |x: &str| p(x).and_then(|pl| pl.template);
        assert_eq!(t("quero ouvir mais alto").as_deref(), Some("Listen louder"));
        assert_eq!(t("make it louder").as_deref(), Some("Listen louder"));
        assert_eq!(t("grave um podcast").as_deref(), Some("Remote podcast"));
        assert_eq!(t("start a live on RTMP").as_deref(), Some("RTMP live"));
        assert_eq!(t("grave meu microfone em FLAC").as_deref(), Some("Clean narration"));
        assert_eq!(t("grave o navegador").as_deref(), Some("Record an app"));
        assert_eq!(t("record the computer sound").as_deref(), Some("Record the computer"));
        assert_eq!(t("grave").as_deref(), Some("Record the computer"), "recording alone records the computer");
        assert_eq!(p("grave meu microfone em FLAC").unwrap().format.as_deref(), Some("FLAC"));
        let l = p("ouvir mais alto").unwrap();
        assert!(!l.record);
        assert_eq!(actions(&l), vec![Action::Create, Action::AskSystem, Action::RunNow]);
        assert_eq!(l.steps[2].title, "Start now");
    }

    #[test]
    fn streams_wait_for_their_server() {
        let pl = p("start a live on RTMP").unwrap();
        assert_eq!(actions(&pl), vec![Action::Create, Action::FillServer]);
    }

    #[test]
    fn transcribing_without_a_transcribe_block_uses_an_automation() {
        let pl = p("grave o computador e transcreva").unwrap();
        assert_eq!(pl.template.as_deref(), Some("Record the computer"));
        assert_eq!(actions(&pl).last(), Some(&Action::Transcribe(false)));
        assert!(transcribe_script().is_some(), "the built-in script that transcribes is there");
    }

    #[test]
    fn a_summary_request_names_its_recording() {
        assert_eq!(quoted("resuma a gravação “Reunião semanal”").as_deref(), Some("Reunião semanal"));
        assert_eq!(quoted("summarize \"Kalango Cast\" please").as_deref(), Some("Kalango Cast"));
        assert_eq!(quoted("resuma a última"), None);
        assert_eq!(p("resuma a gravação “Reunião semanal”").unwrap().name, "Reunião semanal");
    }

    #[test]
    fn stop_summarize_and_the_latest_recording() {
        assert_eq!(actions(&p("pare tudo").unwrap()), vec![Action::StopAll]);
        assert_eq!(actions(&p("stop").unwrap()), vec![Action::StopAll]);
        assert_eq!(actions(&p("resuma a última reunião").unwrap()), vec![Action::OpenLatestTranscript]);
        assert_eq!(actions(&p("summarize the last recording").unwrap()), vec![Action::OpenLatestTranscript]);
        assert_eq!(actions(&p("transcreva a última gravação").unwrap()), vec![Action::TranscribeLatest]);
        assert_eq!(actions(&p("transcribe the latest recording").unwrap()), vec![Action::TranscribeLatest]);
    }

    #[test]
    fn what_cannot_be_done_says_so() {
        assert!(p("faça um café").is_none());
        assert!(p("what is the weather").is_none());
        assert!(p("").is_none());
    }

    #[test]
    fn words_for_when() {
        assert_eq!(when_text(When::Now), "now");
        assert_eq!(when_text(When::Once { day_offset: 1, start: 8 * 60 }), "tomorrow at 08:00");
        assert_eq!(when_text(When::Repeat { days: 0b1111111, start: 20 * 60 }), "every day at 20:00");
        assert_eq!(when_text(When::Repeat { days: 0b1010, start: 9 * 60 }), "every Mon, Wed at 09:00");
    }
}
