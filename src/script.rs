//! Kalango Audio scripts: plain JavaScript (run by boa_engine) with two globals of our own,
//! `kalango` (the app: sessions, notifications, log, quit) and `ctx` (what fired the script:
//! the event kind, its session and, after a recording, the recording file).
//! Scripts run sandboxed; whatever they ask for (start a session, trash a file…) is collected
//! as a list of actions that the app carries out afterwards.
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
    /// open_session, focus, quit, trash, show_in_files, open_editor, open_player, transcribe.
    #[serde(default)]
    pub acts: Vec<Vec<String>>,
    /// (session name, block id, bypassed) for every block whose `bypassed` the script changed
    #[serde(default)]
    pub blocks: Vec<(String, u64, bool)>,
    #[serde(default)]
    pub err: Option<String>,
}

/// What a script says it needs, from a header comment such as `// requer: gravação`.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Needs {
    pub session: bool,
    pub recording: bool,
}

/// Reads the `// requer: …` header lines (accents optional). A recording always comes with its session.
pub fn needs(text: &str) -> Needs {
    let mut n = Needs::default();
    for line in text.lines().map(str::trim).filter(|l| l.starts_with("//")) {
        let l = line.trim_start_matches('/').trim().to_lowercase();
        let Some(rest) = l.strip_prefix("requer:") else { continue };
        if rest.contains("grava") { n.recording = true; n.session = true; }
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

/// `recording.transcribe()`: converts the file to 16 kHz mono with ffmpeg and runs whisper.cpp on it in the
/// background, writing `<recording>.txt` next to it. Returns the text file it will write.
pub fn transcribe_file(path: &str, command: &str, model: &str, language: &str) -> Result<std::path::PathBuf, String> {
    let model = expand_home(model.trim());
    if model.as_os_str().is_empty() { return Err("escolha um modelo whisper (.bin) no bloco Transcrever ou em transcribe({ model })".into()); }
    if !model.is_file() { return Err(format!("o modelo “{}” não existe", model.display())); }
    let src = std::path::PathBuf::from(path);
    if !src.is_file() { return Err(format!("a gravação “{path}” não existe")); }
    let out_base = src.with_extension("");
    let txt = src.with_extension("txt");
    let (command, language) = (if command.trim().is_empty() { "whisper-cli".to_string() } else { command.trim().to_string() }, if language.trim().is_empty() { "auto".to_string() } else { language.trim().to_string() });
    std::thread::spawn(move || {
        let tmp = std::env::temp_dir().join(format!("kalango-transcrever-{}-{}.wav", std::process::id(), crate::model::new_uid()));
        let ok = std::process::Command::new("ffmpeg").args(["-y", "-loglevel", "error", "-i"]).arg(&src).args(["-ar", "16000", "-ac", "1"]).arg(&tmp)
            .stdin(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        if ok {
            let _ = std::process::Command::new(&command).arg("-m").arg(&model).arg("-f").arg(&tmp).args(["-l", &language, "-np", "-otxt", "-of"]).arg(&out_base)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
        let _ = std::fs::remove_file(&tmp);
    });
    Ok(txt)
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
    transcribe: function (opts) {
      opts = opts || {};
      __act('transcribe', r.path, opts.model || '', opts.language || '');
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

/// Example scripts that come with Kalango Audio.
pub const BUILTIN: &[(&str, &str)] = &[
    ("Apagar gravações curtas", r#"// Gravações com menos de 3 segundos costumam ser cliques sem querer: vão para a lixeira.
// requer: gravação

const LIMITE_S = 3;
const g = ctx.recording;
if (!g || g.seconds >= LIMITE_S)
  return;

kalango.notify(`“${g.name}” tinha só ${g.seconds.toFixed(1)} s e foi para a lixeira`);
g.trash();
"#),
    ("Abrir gravação no editor", r#"// Abre a gravação no editor de áudio escolhido nas Preferências.
// requer: gravação

if (ctx.recording) {
  kalango.log(`abrindo ${ctx.recording.name} no editor`);
  ctx.recording.openWith('editor');
}
"#),
    ("Abrir gravação no player", r#"// Toca a gravação no reprodutor padrão do sistema.
// requer: gravação

if (ctx.recording)
  ctx.recording.openWith('player');
"#),
    ("Mostrar gravação no Arquivos", r#"// Abre a pasta da gravação no gerenciador de arquivos.
// requer: gravação

if (ctx.recording)
  ctx.recording.showInFiles();
"#),
    ("Avisar quando a gravação terminar", r#"// Mostra uma notificação com a duração e o tamanho do arquivo novo.
// requer: gravação

const g = ctx.recording;
if (!g)
  return;

const min = Math.floor(g.seconds / 60);
const seg = String(Math.round(g.seconds % 60)).padStart(2, '0');
const mb = (g.bytes / 1048576).toFixed(1);
kalango.notify(`Gravação pronta: ${g.name} (${min}:${seg}, ${mb} MB)`);
"#),
    ("Transcrever ao terminar", r#"// Passa a gravação pelo whisper.cpp e salva o texto ao lado dela (.txt).
// O modelo vem do bloco Transcrever da sessão; ou informe aqui: { model: '~/modelos/ggml-base.bin' }.
// requer: gravação

if (ctx.recording)
  ctx.recording.transcribe({ language: 'pt' });
"#),
    ("Religar blocos da sessão", r#"// Tira todos os blocos da sessão do modo ignorado.
// requer: sessão

if (!ctx.session)
  return;

let n = 0;
for (const b of ctx.session.blocks) {
  if (b.bypassed) { b.bypassed = false; n++; }
}
kalango.log(`${n} bloco(s) religado(s) em ${ctx.session.name}`);
"#),
    ("Parar tudo e sair", r#"// Para todas as sessões (finalizando as gravações) e fecha o Kalango Audio.

kalango.stopAll();
kalango.quit();
"#),
];

pub const HELP: &str = "Scripts do Kalango Audio são JavaScript. Rodam quando uma automação dispara\n\
(ou com “Executar agora”). Use return para encerrar antes do fim.\n\
\n\
ctx — o que disparou o script\n\
  ctx.kind              recording_finished, session_starting, session_stopped,\n\
                        timer_starting, timer_stopped ou manual\n\
  ctx.session           a sessão do evento (ou null)\n\
  ctx.recording         a gravação que acabou de terminar (ou null)\n\
\n\
Sessão\n\
  .name, .running\n\
  .blocks               lista de { id, title, bypassed }; mude bypassed para ignorar\n\
                        ou religar um bloco\n\
  .start(), .stop(), .open()   iniciar, parar, mostrar a sessão na janela\n\
\n\
Gravação\n\
  .path, .name, .seconds (duração), .bytes (tamanho)\n\
  .trash()              manda para a lixeira\n\
  .showInFiles()        abre a pasta no gerenciador de arquivos\n\
  .openWith('editor')   abre no editor de áudio das Preferências\n\
  .openWith('player')   toca no reprodutor padrão\n\
  .transcribe({ model, language })   gera um .txt ao lado (whisper.cpp)\n\
\n\
kalango — o aplicativo\n\
  kalango.sessions()        todas as sessões\n\
  kalango.session(nome)     uma sessão pelo nome (ou null)\n\
  kalango.start(nome), kalango.stop(nome), kalango.stopAll()\n\
  kalango.notify(texto)     notificação na área de trabalho\n\
  kalango.log(texto)        linha no registro de scripts (console.log também serve)\n\
  kalango.focus()           traz o Kalango Audio para a frente\n\
  kalango.quit()            fecha o aplicativo\n\
\n\
Cabeçalho opcional: “// requer: gravação” ou “// requer: sessão”. Com ele, “Executar\n\
agora” entrega a gravação mais recente e a sessão aberta ao script.";

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
        let o = run("const g = ctx.recording; if (g.seconds > 5) return; kalango.notify('curta', g.name); g.trash(); g.showInFiles(); g.openWith('editor'); g.openWith('player'); g.transcribe({ language: 'pt' });", &input());
        assert!(o.err.is_none(), "{:?}", o.err);
        assert_eq!(o.acts[0], vec!["notify".to_string(), "curta a.wav".to_string()]);
        assert_eq!(o.acts[1], vec!["trash".to_string(), "/tmp/a.wav".to_string()]);
        assert_eq!(ids(&o), vec!["notify", "trash", "show_in_files", "open_editor", "open_player", "transcribe"]);
        assert_eq!(o.acts[5], vec!["transcribe", "/tmp/a.wav", "", "pt"]);
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
    }

    #[test]
    fn every_built_in_script_runs() {
        let by = |n: &str| BUILTIN.iter().find(|(b, _)| *b == n).unwrap().1;
        for (name, text) in BUILTIN {
            let o = run(text, &input());
            assert!(o.err.is_none(), "{name}: {:?}", o.err);
        }
        assert_eq!(ids(&run(by("Apagar gravações curtas"), &input())), vec!["notify", "trash"]);
        let mut long = input();
        long.recording.as_mut().unwrap().seconds = 3.0;
        assert!(run(by("Apagar gravações curtas"), &long).acts.is_empty(), "3 s is long enough to keep");
        assert_eq!(ids(&run(by("Parar tudo e sair"), &input())), vec!["stop_all", "quit"]);
        assert_eq!(ids(&run(by("Abrir gravação no editor"), &input())), vec!["log", "open_editor"]);
        assert_eq!(ids(&run(by("Mostrar gravação no Arquivos"), &input())), vec!["show_in_files"]);
        assert_eq!(ids(&run(by("Transcrever ao terminar"), &input())), vec!["transcribe"]);
        assert_eq!(run(by("Avisar quando a gravação terminar"), &input()).acts[0][1], "Gravação pronta: a.wav (0:02, 2.0 MB)");
        assert_eq!(run(by("Religar blocos da sessão"), &input()).blocks, vec![("Show".to_string(), 8, false)]);
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
