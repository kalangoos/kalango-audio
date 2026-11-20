//! Kalango Audio scripts: plain JavaScript (run by boa_engine) with two globals of our own,
//! `kalango` (the app: sessions, notifications, log, quit) and `ctx` (what fired the script:
//! the event kind, its session and, after a recording, the recording file).
//! Scripts run sandboxed; whatever they ask for (start a session, trash a file…) is collected
//! as a list of actions that the app carries out afterwards.
use crate::i18n::{n_, tr};
use boa_engine::{Context, Source};
use serde::{Deserialize, Serialize};

/// A block as a script sees it.
#[derive(Serialize, Clone, Default)]
pub struct BlockInfo {
    pub id: u64,
    pub title: String,
    pub bypassed: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct SessionInfo {
    pub name: String,
    pub running: bool,
    pub blocks: Vec<BlockInfo>,
}

/// A finished recording as a script sees it (`ctx.recording`).
#[derive(Serialize, Clone, Default, Debug)]
pub struct RecordingInfo {
    pub path: String,
    pub name: String,
    pub seconds: f64,
    pub bytes: u64,
}

#[derive(Serialize, Default)]
pub struct Input {
    pub sessions: Vec<SessionInfo>,
    /// event id: recording_finished, session_starting, session_stopped, timer_starting, timer_stopped or manual
    pub kind: String,
    pub session: Option<String>,
    pub recording: Option<RecordingInfo>,
}

#[derive(Deserialize, Default, Debug)]
pub struct Output {
    /// Each action is `[id, args…]`. Ids: log, notify, start_session, stop_session, stop_all,
    /// open_session, focus, quit, trash, show_in_files, open_editor, open_player, transcribe, summarize.
    #[serde(default)]
    pub acts: Vec<Vec<String>>,
    /// (session name, block id, bypassed) for every block whose `bypassed` the script changed
    #[serde(default)]
    pub blocks: Vec<(String, u64, bool)>,
    #[serde(default)]
    pub err: Option<String>,
}

/// What a script says it needs, from a header comment such as `// needs: recording` (older scripts: `// requer: gravação`).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Needs {
    pub session: bool,
    pub recording: bool,
}

/// Reads the `// needs: …` (or `// requer: …`) header lines (accents optional). A recording always comes with its
/// session.
pub fn needs(text: &str) -> Needs {
    let mut n = Needs::default();
    for line in text.lines().map(str::trim).filter(|l| l.starts_with("//")) {
        let l = line.trim_start_matches('/').trim().to_lowercase();
        let Some(rest) = l.strip_prefix("needs:").or_else(|| l.strip_prefix("requer:")) else { continue };
        if rest.contains("grava") || rest.contains("recording") { n.recording = true; n.session = true; }
        if rest.contains("sess") { n.session = true; }
    }
    n
}

static FOCUS_REQUEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `kalango.focus()`: asks the window to come to the front on the next frame.
pub fn request_focus() { FOCUS_REQUEST.store(true, std::sync::atomic::Ordering::Relaxed); }
/// True once after `request_focus`; the UI then raises its window.
pub fn take_focus_request() -> bool { FOCUS_REQUEST.swap(false, std::sync::atomic::Ordering::Relaxed) }

/// `kalango.notify(text)`: a desktop notification through `notify-send`. False when it could not be shown.
pub fn desktop_notify(text: &str) -> bool {
    std::process::Command::new("notify-send").args(["-a", "Kalango Audio", "Kalango Audio", text]).spawn().is_ok()
}

fn expand_home(p: &str) -> std::path::PathBuf {
    match p.strip_prefix('~') {
        Some(r) => std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(r.trim_start_matches('/')),
        None => std::path::PathBuf::from(p),
    }
}

const PRELUDE: &str = r#"
var __acts = [];
function __act() { var a = []; for (var i = 0; i < arguments.length; i++) a.push(String(arguments[i])); __acts.push(a); }
function __text(args) { var p = []; for (var i = 0; i < args.length; i++) p.push(typeof args[i] === 'object' ? JSON.stringify(args[i]) : String(args[i])); return p.join(' '); }
var __sessions = INPUT.sessions.map(function (s) {
  return {
    name: s.name,
    running: s.running,
    blocks: s.blocks.map(function (b) { return { id: b.id, title: b.title, bypassed: b.bypassed, __was: b.bypassed }; }),
    start: function () { __act('start_session', s.name); },
    stop: function () { __act('stop_session', s.name); },
    open: function () { __act('open_session', s.name); }
  };
});
function __recording(r) {
  if (!r) return null;
  return {
    path: r.path, name: r.name, seconds: r.seconds, bytes: r.bytes,
    trash: function () { __act('trash', r.path); },
    showInFiles: function () { __act('show_in_files', r.path); },
    openWith: function (what) {
      var w = String(what === undefined ? 'player' : what);
      if (w !== 'editor' && w !== 'player') throw new Error("openWith aceita 'editor' ou 'player', não '" + w + "'");
      __act(w === 'editor' ? 'open_editor' : 'open_player', r.path);
    },
    transcribe: function () {
      __act('transcribe', r.path);
    },
    summarize: function () {
      __act('summarize', r.path);
    }
  };
}
var kalango = {
  sessions: function () { return __sessions.slice(); },
  session: function (name) { for (var i = 0; i < __sessions.length; i++) if (__sessions[i].name === String(name)) return __sessions[i]; return null; },
  start: function (name) { __act('start_session', name); },
  stop: function (name) { __act('stop_session', name); },
  stopAll: function () { __act('stop_all'); },
  focus: function () { __act('focus'); },
  quit: function () { __act('quit'); },
  notify: function () { __act('notify', __text(arguments)); },
  log: function () { __act('log', __text(arguments)); }
};
var console = {
  log: function () { __act('log', __text(arguments)); },
  warn: function () { __act('log', 'aviso: ' + __text(arguments)); },
  error: function () { __act('log', 'erro: ' + __text(arguments)); }
};
var ctx = {
  kind: INPUT.kind,
  session: INPUT.session ? kalango.session(INPUT.session) : null,
  recording: __recording(INPUT.recording)
};
var __err = null;
"#;

pub fn run(text: &str, input: &Input) -> Output {
    let json = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
    let code = format!(
        "var INPUT = {json};\n{PRELUDE}\ntry {{ (function (kalango, ctx, console) {{\n{text}\n}})(kalango, ctx, console); }} catch (e) {{ __err = String(e); }}\n\
         var __changed = [];\n__sessions.forEach(function (s) {{ s.blocks.forEach(function (b) {{ if (!!b.bypassed !== !!b.__was) __changed.push([s.name, b.id, !!b.bypassed]); }}); }});\n\
         JSON.stringify({{ acts: __acts, err: __err, blocks: __changed }});"
    );
    let mut ctx = Context::default();
    ctx.runtime_limits_mut().set_loop_iteration_limit(5_000_000);
    ctx.runtime_limits_mut().set_recursion_limit(400);
    match ctx.eval(Source::from_bytes(code.as_bytes())) {
        Ok(v) => match v.to_string(&mut ctx) {
            Ok(s) => serde_json::from_str(&s.to_std_string_escaped()).unwrap_or_else(|e| Output { err: Some(format!("não foi possível ler o resultado: {e}")), ..Default::default() }),
            Err(e) => Output { err: Some(e.to_string()), ..Default::default() },
        },
        Err(e) => Output { err: Some(format!("{e}")), ..Default::default() },
    }
}

/// Example scripts that come with Kalango Audio. Names are English msgids (shown with `tr`); automations store
/// the English name. Keep "Stop everything and quit" last (the core's quit test runs the last one).
pub const BUILTIN: &[(&str, &str)] = &[
    (n_("Delete short recordings"), r#"// Recordings shorter than 3 seconds are usually accidental clicks: they go to the trash.
// needs: recording

const LIMIT_S = 3;
const r = ctx.recording;
if (!r || r.seconds >= LIMIT_S)
  return;

kalango.notify(`“${r.name}” was only ${r.seconds.toFixed(1)} s long and went to the trash`);
r.trash();
"#),
    (n_("Open the recording in the editor"), r#"// Opens the recording in the audio editor chosen in Settings.
// needs: recording

if (ctx.recording) {
  kalango.log(`opening ${ctx.recording.name} in the editor`);
  ctx.recording.openWith('editor');
}
"#),
    (n_("Play the recording in the player"), r#"// Plays the recording in the system's default player.
// needs: recording

if (ctx.recording)
  ctx.recording.openWith('player');
"#),
    (n_("Show the recording in Files"), r#"// Opens the recording's folder in the file manager.
// needs: recording

if (ctx.recording)
  ctx.recording.showInFiles();
"#),
    (n_("Notify when a recording finishes"), r#"// Shows a notification with the new file's length and size.
// needs: recording

const r = ctx.recording;
if (!r)
  return;

const min = Math.floor(r.seconds / 60);
const sec = String(Math.round(r.seconds % 60)).padStart(2, '0');
const mb = (r.bytes / 1048576).toFixed(1);
kalango.notify(`Recording ready: ${r.name} (${min}:${sec}, ${mb} MB)`);
"#),
    (n_("Transcribe when done"), r#"// Turns the recording into text with KalangoOS's local voice model and saves it next to it (.txt),
// one "[mm:ss] text" line per stretch of speech. The language is the system's.
// needs: recording

if (ctx.recording)
  ctx.recording.transcribe();
"#),
    (n_("Switch the session’s blocks back on"), r#"// Takes every block of the session out of bypass.
// needs: session

if (!ctx.session)
  return;

let n = 0;
for (const b of ctx.session.blocks) {
  if (b.bypassed) { b.bypassed = false; n++; }
}
kalango.log(`${n} block(s) switched back on in ${ctx.session.name}`);
"#),
    (n_("Stop everything and quit"), r#"// Stops every session (finishing the recordings) and closes Kalango Audio.

kalango.stopAll();
kalango.quit();
"#),
];

/// The names the built-in scripts had before they became English msgids (older saved automations use them).
pub const LEGACY_NAMES: &[(&str, &str)] = &[
    ("Apagar gravações curtas", "Delete short recordings"),
    ("Abrir gravação no editor", "Open the recording in the editor"),
    ("Abrir gravação no player", "Play the recording in the player"),
    ("Mostrar gravação no Arquivos", "Show the recording in Files"),
    ("Avisar quando a gravação terminar", "Notify when a recording finishes"),
    ("Transcrever ao terminar", "Transcribe when done"),
    ("Religar blocos da sessão", "Switch the session’s blocks back on"),
    ("Parar tudo e sair", "Stop everything and quit"),
];

/// The current name of a script an automation refers to (an old built-in name becomes the English one).
pub fn current_name(name: &str) -> &str {
    LEGACY_NAMES.iter().find(|(old, _)| *old == name).map(|(_, new)| *new).unwrap_or(name)
}

/// The script API reference shown by "Script help" (translated; code stays as it is).
pub fn help() -> String {
    let rows: &[(&str, &str)] = &[
        ("", tr("Kalango Audio scripts are JavaScript. They run when an automation fires (or with “Run now”). Use return to finish early.")),
        ("", ""),
        ("ctx", tr("what fired the script")),
        ("  ctx.kind", tr("recording_finished, session_starting, session_stopped, timer_starting, timer_stopped or manual")),
        ("  ctx.session", tr("the event’s session (or null)")),
        ("  ctx.recording", tr("the recording that just finished (or null)")),
        ("", ""),
        (tr("Session"), ""),
        ("  .name, .running", ""),
        ("  .blocks", tr("list of { id, title, bypassed }; change bypassed to bypass a block or switch it back on")),
        ("  .start(), .stop(), .open()", tr("start, stop, show the session in the window")),
        ("", ""),
        (tr("Recording"), ""),
        ("  .path, .name, .seconds, .bytes", tr("file, name, length and size")),
        ("  .trash()", tr("moves it to the trash")),
        ("  .showInFiles()", tr("opens its folder in the file manager")),
        ("  .openWith('editor')", tr("opens it in the audio editor from Settings")),
        ("  .openWith('player')", tr("plays it in the default player")),
        ("  .transcribe()", tr("writes a .txt next to it (KalangoOS voice model)")),
        ("  .summarize()", tr("transcribes it if needed, then writes a .resumo.md summary next to it (local model)")),
        ("", ""),
        ("kalango", tr("the app")),
        ("  kalango.sessions()", tr("every session")),
        ("  kalango.session(name)", tr("a session by name (or null)")),
        ("  kalango.start(name), kalango.stop(name), kalango.stopAll()", ""),
        ("  kalango.notify(text)", tr("a desktop notification")),
        ("  kalango.log(text)", tr("a line in the script log (console.log works too)")),
        ("  kalango.focus()", tr("brings Kalango Audio to the front")),
        ("  kalango.quit()", tr("closes the app")),
        ("", ""),
        ("", tr("Optional header: “// needs: recording” or “// needs: session”. With it, “Run now” gives the script the latest recording and the open session.")),
    ];
    let mut out = String::new();
    for (code, text) in rows {
        match (code.is_empty(), text.is_empty()) {
            (true, true) => out.push('\n'),
            (true, false) => out.push_str(&format!("{text}\n")),
            (false, true) => out.push_str(&format!("{code}\n")),
            (false, false) => out.push_str(&format!("{code:<34} {text}\n")),
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Input {
        Input {
            sessions: vec![SessionInfo { name: "Show".into(), running: true, blocks: vec![BlockInfo { id: 7, title: "Gravador".into(), bypassed: false }, BlockInfo { id: 8, title: "Saída de áudio".into(), bypassed: true }] }],
            kind: "recording_finished".into(),
            session: Some("Show".into()),
            recording: Some(RecordingInfo { path: "/tmp/a.wav".into(), name: "a.wav".into(), seconds: 2.0, bytes: 2_097_152 }),
        }
    }

    fn ids(o: &Output) -> Vec<&str> { o.acts.iter().map(|a| a[0].as_str()).collect() }

    #[test]
    fn scripts_see_the_trigger_and_ask_for_actions() {
        let o = run("const g = ctx.recording; if (g.seconds > 5) return; kalango.notify('curta', g.name); g.trash(); g.showInFiles(); g.openWith('editor'); g.openWith('player'); g.transcribe();", &input());
        assert!(o.err.is_none(), "{:?}", o.err);
        assert_eq!(o.acts[0], vec!["notify".to_string(), "curta a.wav".to_string()]);
        assert_eq!(o.acts[1], vec!["trash".to_string(), "/tmp/a.wav".to_string()]);
        assert_eq!(ids(&o), vec!["notify", "trash", "show_in_files", "open_editor", "open_player", "transcribe"]);
        assert_eq!(o.acts[5], vec!["transcribe", "/tmp/a.wav"]);
        let o = run("if (ctx.recording.seconds > 1) return; kalango.quit();", &input());
        assert!(o.acts.is_empty());
        let o = run("kalango.log(ctx.kind, ctx.session.name, ctx.session.running, ctx.recording.bytes); console.log('x');", &input());
        assert_eq!(o.acts, vec![vec!["log".to_string(), "recording_finished Show true 2097152".to_string()], vec!["log".to_string(), "x".to_string()]]);
        let o = run("ctx.recording.openWith('banana');", &input());
        assert!(o.err.as_deref().unwrap_or("").contains("openWith"), "{:?}", o.err);
    }

    #[test]
    fn scripts_can_bypass_blocks_and_control_sessions() {
        let o = run("ctx.session.blocks.forEach(b => { b.bypassed = b.title == 'Gravador'; }); kalango.stopAll(); kalango.session('Show').stop(); kalango.start('Other'); kalango.session('Show').open(); kalango.focus(); kalango.stop('Show');", &input());
        assert!(o.err.is_none(), "{:?}", o.err);
        assert_eq!(o.blocks, vec![("Show".to_string(), 7, true), ("Show".to_string(), 8, false)]);
        assert_eq!(ids(&o), vec!["stop_all", "stop_session", "start_session", "open_session", "focus", "stop_session"]);
        let o = run("if (kalango.session('Nope') !== null || kalango.sessions().length !== 1) throw new Error('bad');", &input());
        assert!(o.err.is_none(), "{:?}", o.err);
    }

    #[test]
    fn broken_scripts_report_errors_instead_of_crashing() {
        let o = run("this is not javascript (", &input());
        assert!(o.err.is_some());
        let o = run("undefinedFunction();", &input());
        assert!(o.err.as_deref().unwrap_or("").contains("undefinedFunction"), "{:?}", o.err);
        let o = run("while (true) {}", &input());
        assert!(o.err.is_some(), "an endless loop must be stopped");
    }

    #[test]
    fn header_comments_say_what_a_script_needs() {
        assert_eq!(needs("// requer: gravação\nfoo()"), Needs { session: true, recording: true });
        assert_eq!(needs("// Requer: sessao"), Needs { session: true, recording: false });
        assert_eq!(needs("kalango.quit(); // requer: gravação no meio não conta? conta só se a linha começa com //"), Needs::default());
        assert_eq!(needs(""), Needs::default());
        assert_eq!(needs("// needs: recording"), Needs { session: true, recording: true });
        assert_eq!(needs("// Needs: session"), Needs { session: true, recording: false });
    }

    #[test]
    fn old_built_in_names_map_to_the_new_ones() {
        for (old, new) in LEGACY_NAMES {
            assert_eq!(current_name(old), *new);
            assert!(BUILTIN.iter().any(|(n, _)| n == new), "{new} is not a built-in");
        }
        assert_eq!(current_name("Mine"), "Mine");
        assert_eq!(BUILTIN.last().unwrap().0, "Stop everything and quit");
        assert!(help().contains("ctx.recording") && help().contains("kalango.quit()"));
    }

    #[test]
    fn every_built_in_script_runs() {
        let by = |n: &str| BUILTIN.iter().find(|(b, _)| *b == n).unwrap().1;
        for (name, text) in BUILTIN {
            let o = run(text, &input());
            assert!(o.err.is_none(), "{name}: {:?}", o.err);
        }
        assert_eq!(ids(&run(by("Delete short recordings"), &input())), vec!["notify", "trash"]);
        let mut long = input();
        long.recording.as_mut().unwrap().seconds = 3.0;
        assert!(run(by("Delete short recordings"), &long).acts.is_empty(), "3 s is long enough to keep");
        assert_eq!(ids(&run(by("Stop everything and quit"), &input())), vec!["stop_all", "quit"]);
        assert_eq!(ids(&run(by("Open the recording in the editor"), &input())), vec!["log", "open_editor"]);
        assert_eq!(ids(&run(by("Show the recording in Files"), &input())), vec!["show_in_files"]);
        assert_eq!(ids(&run(by("Transcribe when done"), &input())), vec!["transcribe"]);
        assert_eq!(run(by("Notify when a recording finishes"), &input()).acts[0][1], "Recording ready: a.wav (0:02, 2.0 MB)");
        assert_eq!(run(by("Switch the session’s blocks back on"), &input()).blocks, vec![("Show".to_string(), 8, false)]);
        let mut none = input();
        none.recording = None;
        none.session = None;
        for (name, text) in BUILTIN {
            let n = needs(text);
            if n.session || n.recording {
                let o = run(text, &none);
                assert!(o.err.is_none() && o.acts.is_empty() && o.blocks.is_empty(), "{name} should do nothing without what it needs");
            }
        }
    }
}
