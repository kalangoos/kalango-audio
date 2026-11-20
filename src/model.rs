//! Session data: blocks on a grid, the wires between them, templates and saving.
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
    ("recording_finished", "Gravação terminada"),
    ("session_starting", "Sessão iniciada"),
    ("session_stopped", "Sessão parada"),
    ("timer_starting", "Agendamento começou"),
    ("timer_stopped", "Agendamento terminou"),
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
    KindInfo { id: "app_source", title: "Aplicativo", desc: "Pega o som de um programa aberto", cat: Cat::Source, working: true },
    KindInfo { id: "device_input", title: "Microfone ou entrada", desc: "Pega o som de um microfone ou interface de áudio", cat: Cat::Source, working: true },
    KindInfo { id: "system_sound", title: "Som do sistema", desc: "Pega tudo o que o computador está tocando", cat: Cat::Source, working: true },
    KindInfo { id: "test_tone", title: "Tom de teste", desc: "Gera um tom contínuo para testar a sessão", cat: Cat::Source, working: true },
    KindInfo { id: "device_output", title: "Saída de áudio", desc: "Toca o som em alto-falantes ou fones", cat: Cat::Output, working: true },
    KindInfo { id: "recorder", title: "Gravador", desc: "Salva o som em um arquivo", cat: Cat::Output, working: true },
    KindInfo { id: "rtmp_stream", title: "Transmissão RTMP", desc: "Envia o som ao vivo para um servidor RTMP", cat: Cat::Output, working: true },
    KindInfo { id: "radio_stream", title: "Rádio online", desc: "Envia o som para um servidor Icecast ou compatível", cat: Cat::Output, working: true },
    KindInfo { id: "graphic_eq", title: "Equalizador gráfico", desc: "Dez faixas de frequência, de 31 Hz a 16 kHz", cat: Cat::Effect, working: true },
    KindInfo { id: "balance", title: "Balanço", desc: "Equilibra os lados esquerdo e direito", cat: Cat::Effect, working: true },
    KindInfo { id: "tone_control", title: "Graves e agudos", desc: "Dois controles simples de tonalidade", cat: Cat::Effect, working: true },
    KindInfo { id: "channels", title: "Canais", desc: "Troca, junta ou isola os canais", cat: Cat::Effect, working: true },
    KindInfo { id: "low_pass", title: "Passa-baixas", desc: "Corta as frequências altas", cat: Cat::Effect, working: true },
    KindInfo { id: "loudness", title: "Encorpar", desc: "Compressão suave que deixa o som mais cheio", cat: Cat::Effect, working: true },
    KindInfo { id: "stereo_widen", title: "Alargar estéreo", desc: "Abre um sinal mono entre os dois lados", cat: Cat::Effect, working: true },
    KindInfo { id: "pan", title: "Panorama", desc: "Posiciona o som entre esquerda e direita", cat: Cat::Effect, working: true },
    KindInfo { id: "compressor", title: "Compressor", desc: "Segura os picos e levanta as partes baixas", cat: Cat::Effect, working: true },
    KindInfo { id: "volume", title: "Volume", desc: "Aumenta ou diminui o volume", cat: Cat::Effect, working: true },
    KindInfo { id: "level_meter", title: "Medidor de nível", desc: "Mostra pico e média em barras", cat: Cat::Meter, working: true },
    KindInfo { id: "needle_meter", title: "Medidor de ponteiro", desc: "Mostra o nível em um ponteiro", cat: Cat::Meter, working: true },
    KindInfo { id: "click_removal", title: "Remover cliques", desc: "Tira estalos curtos do som", cat: Cat::Advanced, working: true },
    KindInfo { id: "hum_removal", title: "Remover zumbido", desc: "Tira o zumbido da rede elétrica (50/60 Hz)", cat: Cat::Advanced, working: true },
    KindInfo { id: "source_switch", title: "Alternar fonte", desc: "Escolhe qual de duas fontes passa adiante", cat: Cat::Advanced, working: true },
    KindInfo { id: "transcribe", title: "Transcrever", desc: "Converte fala em texto enquanto grava", cat: Cat::Advanced, working: true },
    KindInfo { id: "ladspa_plugin", title: "Plug-in LADSPA", desc: "Usa um efeito LADSPA instalado no sistema", cat: Cat::Advanced, working: true },
    KindInfo { id: "voice_denoise", title: "Limpar voz", desc: "Remove ruído de fundo da fala (RNNoise)", cat: Cat::Advanced, working: true },
    KindInfo { id: "convolution", title: "Convolução", desc: "Aplica uma resposta ao impulso (FIR) de um arquivo", cat: Cat::Advanced, working: true },
    KindInfo { id: "noise_reduction", title: "Redução de ruído", desc: "Aprende o ruído de fundo e o diminui", cat: Cat::Advanced, working: true },
    KindInfo { id: "ducking", title: "Abaixar sob voz", desc: "Abaixa as outras fontes quando alguém fala", cat: Cat::Advanced, working: true },
    KindInfo { id: "mixer", title: "Mixer", desc: "Mistura até cinco fontes", cat: Cat::Advanced, working: true },
    KindInfo { id: "parametric_eq", title: "EQ paramétrico", desc: "Até 16 faixas com frequência, ganho e Q", cat: Cat::Advanced, working: true },
    KindInfo { id: "delay", title: "Atraso", desc: "Atrasa o som em milissegundos para sincronizar", cat: Cat::Advanced, working: true },
    KindInfo { id: "time_shift", title: "Voltar no tempo", desc: "Pausa e volta o som ao vivo", cat: Cat::Advanced, working: true },
    KindInfo { id: "high_pass", title: "Passa-altas", desc: "Corta as frequências baixas", cat: Cat::Effect, working: true },
    KindInfo { id: "band_pass", title: "Passa-faixa", desc: "Deixa passar só uma faixa de frequências", cat: Cat::Effect, working: true },
    KindInfo { id: "high_shelf", title: "Realce de agudos", desc: "Aumenta ou corta acima de uma frequência", cat: Cat::Effect, working: true },
    KindInfo { id: "low_shelf", title: "Realce de graves", desc: "Aumenta ou corta abaixo de uma frequência", cat: Cat::Effect, working: true },
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
        let mut s = Session::new(&self.name);
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
    ["Gravar o computador", "Narração limpa", "Reunião online"]
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
