//! The audio graph: runs the blocks of a session in order, ten milliseconds at a time.
use crate::audio::{Stream, CHUNK};
use crate::dsp::{db_to_lin, lin_to_db, Dsp, Frame, SR};
use crate::media::Tags;
use crate::model::{is_source, Block};
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
pub struct Meter { pub peak: [f32; 2], pub rms: [f32; 2] }

#[derive(Clone, Default)]
pub struct RecStatus {
    pub file: String,
    pub seconds: f64,
    pub bytes: u64,
    pub state: String, // "recording", "paused", "silent", "finished", "error: ..."
}

#[derive(Default)]
pub struct Shared {
    pub blocks: Mutex<Vec<Block>>,
    pub session_name: Mutex<String>,
    pub meters: Mutex<HashMap<u64, Meter>>,
    pub rec: Mutex<HashMap<u64, RecStatus>>,
    pub errors: Mutex<HashMap<u64, String>>,
    pub events: Mutex<Vec<FileEnd>>,
    /// the session's sample rate for recordings (0 = 48000)
    pub session_rate: std::sync::atomic::AtomicU32,
    /// how busy the audio thread is, in thousandths of the time it has for each 10 ms of sound
    pub load: std::sync::atomic::AtomicU32,
    /// chunks that took longer than the sound they carried (the sound may have glitched)
    pub overruns: std::sync::atomic::AtomicU32,
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    pub shared: Arc<Shared>,
}

impl Engine {
    pub fn start(blocks: Vec<Block>, name: &str) -> Engine { Engine::start_with_rate(blocks, name, 0) }

    pub fn start_with_rate(blocks: Vec<Block>, name: &str, rate: u32) -> Engine {
        let shared = Arc::new(Shared::default());
        shared.session_rate.store(rate, Ordering::Relaxed);
        *shared.blocks.lock().unwrap() = blocks;
        *shared.session_name.lock().unwrap() = name.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, st) = (shared.clone(), stop.clone());
        let handle = std::thread::spawn(move || run(sh, st));
        Engine { stop, handle: Some(handle), shared }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
}

// ---------------- recording ----------------

enum Writer {
    Wav(hound::WavWriter<BufWriter<std::fs::File>>),
    Aiff { f: BufWriter<std::fs::File>, frames: u32, bits: u16, rate: u32, channels: u16 },
    Ffmpeg { child: std::process::Child, stdin: Option<std::process::ChildStdin> },
}

/// Linear resampler for the recorder's sample-rate choice.
struct Resampler {
    ratio: f32, // input frames per output frame
    pos: f32,
    last: Frame,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self { Resampler { ratio: from as f32 / to as f32, pos: 0.0, last: [0.0; 2] } }
    fn run(&mut self, input: &[Frame]) -> Vec<Frame> {
        if (self.ratio - 1.0).abs() < 1e-6 { return input.to_vec(); }
        let mut out = vec![];
        // position 0 is the previous chunk's last frame, 1.. are this chunk's frames
        while self.pos < input.len() as f32 {
            let i = self.pos.floor() as isize;
            let fr = self.pos - self.pos.floor();
            let a = if i < 0 { self.last } else { input[i as usize] };
            let b = input[((i + 1).max(0) as usize).min(input.len() - 1)];
            out.push([a[0] + (b[0] - a[0]) * fr, a[1] + (b[1] - a[1]) * fr]);
            self.pos += self.ratio;
        }
        self.pos -= input.len() as f32;
        if let Some(l) = input.last() { self.last = *l; }
        out
    }
}

struct Recording {
    writer: Writer,
    path: String,
    frames: u64,
    bytes: u64,
    rng: u64,
    err: [f32; 2],
    channels: u16,
    resamp: Resampler,
    tags: Tags,
}

/// A file that has just been finished (it raises the "recording finished" automation event).
#[derive(Clone, Debug)]
pub struct FileEnd {
    pub block: u64,
    pub path: String,
    pub seconds: f64,
    pub size: u64,
}

pub fn tags_from_params(p: &crate::model::Params) -> Tags {
    Tags {
        title: p.text("tag_title", ""), artist: p.text("tag_artist", ""), album: p.text("tag_album", ""), genre: p.text("tag_genre", ""),
        comment: p.text("tag_comment", ""), year: p.text("tag_year", ""), track: p.text("tag_track", ""), track_count: p.text("tag_trackcount", ""),
        disc: p.text("tag_disc", ""), disc_count: p.text("tag_disccount", ""), compilation: p.num("tag_compilation", 0.0) > 0.5, art: p.text("tag_art", ""),
    }
}

pub fn ffmpeg_available() -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join("ffmpeg").is_file())).unwrap_or(false)
}

pub fn format_ext(fmt: &str) -> &'static str {
    match fmt { "WAV" => "wav", "AIFF" => "aiff", "MP3" => "mp3", "AAC" | "ALAC" => "m4a", "FLAC" => "flac", _ => "wav" }
}

/// A user folder from the XDG user-dirs setup (`MUSIC`, `DOCUMENTS`…): the environment variable, then
/// `~/.config/user-dirs.dirs`, then `xdg-user-dir`, then the first of `fallbacks` that exists under the home folder
/// (or the first fallback when none exists).
fn xdg_user_dir(kind: &str, fallbacks: &[&str]) -> std::path::PathBuf {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let expand = |v: &str| -> Option<std::path::PathBuf> {
        let v = v.trim().trim_matches('"');
        if v.is_empty() { return None; }
        let p = if let Some(r) = v.strip_prefix("$HOME") { home.join(r.trim_start_matches('/')) } else { std::path::PathBuf::from(v) };
        // a user dir that points at the home folder itself means "not set"
        if p == home || !p.is_absolute() { None } else { Some(p) }
    };
    let var = format!("XDG_{kind}_DIR");
    if let Some(p) = std::env::var(&var).ok().and_then(|v| expand(&v)) { return p; }
    let cfg = std::env::var("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| home.join(".config"));
    if let Ok(text) = std::fs::read_to_string(cfg.join("user-dirs.dirs")) {
        for l in text.lines() {
            if let Some(v) = l.trim().strip_prefix(&format!("{var}=")) { if let Some(p) = expand(v) { return p; } }
        }
    }
    if let Ok(o) = std::process::Command::new("xdg-user-dir").arg(kind).stderr(std::process::Stdio::null()).output() {
        if let Some(p) = expand(&String::from_utf8_lossy(&o.stdout)) { return p; }
    }
    fallbacks.iter().map(|f| home.join(f)).find(|p| p.is_dir()).unwrap_or_else(|| home.join(fallbacks[0]))
}

/// Writes a path under the home folder as "~/…", which is how folders are kept in sessions and settings.
fn tilde(p: &std::path::Path) -> String {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
    match p.strip_prefix(&home) {
        Ok(r) if !home.as_os_str().is_empty() => format!("~/{}", r.display()),
        _ => p.display().to_string(),
    }
}

/// Where recordings go unless a Recorder block says otherwise: the user's music folder + "Kalango Audio".
pub fn default_recordings_dir() -> String {
    // tests never write into the real home folder
    #[cfg(test)]
    {
        test_home().join("Música/Kalango Audio").display().to_string()
    }
    #[cfg(not(test))]
    {
        static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        DIR.get_or_init(|| tilde(&xdg_user_dir("MUSIC", &["Música", "Music"]).join("Kalango Audio"))).clone()
    }
}

/// The user's documents folder (for exported sessions), as "~/…".
pub fn documents_dir() -> String {
    #[cfg(test)]
    {
        test_home().join("Documentos").display().to_string()
    }
    #[cfg(not(test))]
    {
        static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        DIR.get_or_init(|| tilde(&xdg_user_dir("DOCUMENTS", &["Documentos", "Documents"]))).clone()
    }
}

/// A scratch "home" for tests, under the system temp folder.
#[cfg(test)]
pub fn test_home() -> std::path::PathBuf { std::env::temp_dir().join(format!("kalango-audio-test-{}", std::process::id())) }

/// Where transcriptions go by default: the documents folder + "Kalango Audio".
pub fn default_transcripts_dir() -> String { format!("{}/Kalango Audio", documents_dir()) }

/// Default name of a recording (tokens are replaced when the file is opened).
pub const DEFAULT_FILE_NAME: &str = "Gravação %date %time";
/// Default name of a transcription file.
pub const DEFAULT_TRANSCRIPT_NAME: &str = "Transcrição %date %time";

fn expand_home(p: &str) -> std::path::PathBuf {
    if let Some(r) = p.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        return std::path::PathBuf::from(home).join(r.trim_start_matches('/'));
    }
    std::path::PathBuf::from(p)
}

/// Replace %name, %date, %time, %counter, the tag tokens and friends in a file name.
pub fn expand_tokens(tpl: &str, session: &str, counter: u32) -> String { expand_tokens_with(tpl, session, counter, &Tags::default()) }

pub fn expand_tokens_with(tpl: &str, session: &str, counter: u32, tags: &Tags) -> String {
    let (y, mo, d, h, mi, s) = local_time();
    let mut out = tpl
        .replace("%prefixed_counter", &format!("{counter:03}"))
        .replace("%counter", &counter.to_string())
        .replace("%name", session)
        .replace("%date", &format!("{y:04}-{mo:02}-{d:02}"))
        .replace("%year", &format!("{y:04}"))
        .replace("%month", &format!("{mo:02}"))
        .replace("%day", &format!("{d:02}"))
        .replace("%time", &format!("{h:02}.{mi:02}.{s:02}"))
        .replace("%hour", &format!("{h:02}"))
        .replace("%minute", &format!("{mi:02}"))
        .replace("%second", &format!("{s:02}"));
    for (t, v) in [
        ("%tag_trackcount", &tags.track_count), ("%tag_track", &tags.track), ("%tag_disccount", &tags.disc_count), ("%tag_disc", &tags.disc),
        ("%tag_title", &tags.title), ("%tag_artist", &tags.artist), ("%tag_album", &tags.album), ("%tag_genre", &tags.genre),
        ("%tag_comment", &tags.comment), ("%tag_year", &tags.year),
    ] {
        out = out.replace(t, v);
    }
    out.chars().map(|c| if c == '/' || c == '\\' || c == ':' { '-' } else { c }).collect::<String>().trim().to_string()
}

/// Weekday (0 = Sunday) and minutes after midnight, local time.
pub fn local_now() -> (u32, u32) {
    unsafe {
        let t: libc::time_t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_wday as u32, (tm.tm_hour * 60 + tm.tm_min) as u32)
    }
}

/// Today as yyyymmdd, local time.
pub fn local_date() -> u32 {
    let (y, m, d, _, _, _) = local_time();
    y as u32 * 10000 + m * 100 + d
}

fn local_time() -> (i32, u32, u32, u32, u32, u32) {
    unsafe {
        let t: libc::time_t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32)
    }
}

fn unique_path(dir: &std::path::Path, name: &str, ext: &str) -> std::path::PathBuf {
    let mut p = dir.join(format!("{name}.{ext}"));
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{name} {i}.{ext}"));
        i += 1;
    }
    p
}

/// 80-bit IEEE extended float, as AIFF stores its sample rate.
fn ieee_extended(rate: u32) -> [u8; 10] {
    let mut e = 0u16;
    let mut m = rate as u64;
    if m == 0 { return [0; 10]; }
    while m & (1 << 63) == 0 { m <<= 1; e += 1; }
    let exp = 16383 + 63 - e;
    let mut b = [0u8; 10];
    b[..2].copy_from_slice(&exp.to_be_bytes());
    b[2..].copy_from_slice(&m.to_be_bytes());
    b
}

fn open_recording(b: &Block, session: &str, counter: u32, session_rate: u32) -> Result<Recording, String> {
    let p = &b.p;
    let tags = tags_from_params(p);
    let dir = expand_home(&p.text("folder", &default_recordings_dir()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("não foi possível criar a pasta: {e}"))?;
    let name = expand_tokens_with(&p.text("file_name", DEFAULT_FILE_NAME), session, counter, &tags);
    let mut fmt = p.text("format", "WAV");
    if matches!(fmt.as_str(), "MP3" | "AAC" | "ALAC" | "FLAC") && !ffmpeg_available() { fmt = "WAV".into(); }
    // Advanced Quality Options: sample rate (0 = same as the session), channels (0 = stereo), container, bit rate mode
    let rate = match p.num("q_rate", 0.0) as u32 { 0 => if session_rate == 0 { 48000 } else { session_rate }, r => r };
    let channels = match p.num("q_channels", 0.0) as u16 { 1 => 1, _ => 2 };
    let mut ext = format_ext(&fmt);
    if matches!(fmt.as_str(), "AAC" | "ALAC") && p.num("q_mp4", 0.0) > 0.5 { ext = "mp4"; }
    let path = unique_path(&dir, if name.is_empty() { "Gravação" } else { &name }, ext);
    let ps = path.to_string_lossy().to_string();
    let depth = p.num("depth", 16.0) as u16;
    let writer = match fmt.as_str() {
        "WAV" => {
            let spec = hound::WavSpec {
                channels, sample_rate: rate,
                bits_per_sample: if depth == 32 { 32 } else if depth == 24 { 24 } else { 16 },
                sample_format: if depth == 32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
            };
            Writer::Wav(hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?)
        }
        "AIFF" => {
            let mut f = BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
            f.write_all(&[0u8; 54]).map_err(|e| e.to_string())?;
            Writer::Aiff { f, frames: 0, bits: if depth >= 24 { 24 } else { 16 }, rate, channels }
        }
        other => {
            let br = format!("{}k", p.num("bitrate", 256.0) as i32);
            let mut codec: Vec<String> = match other {
                "MP3" => if p.num("q_vbr", 0.0) > 0.5 {
                    // variable bit rate: quality follows the chosen bit-rate class
                    let q = match p.num("bitrate", 256.0) as i32 { ..=64 => "6", 65..=160 => "4", _ => "0" };
                    vec!["-c:a".into(), "libmp3lame".into(), "-q:a".into(), q.into()]
                } else { vec!["-c:a".into(), "libmp3lame".into(), "-b:a".into(), br] },
                "AAC" => vec!["-c:a".into(), "aac".into(), "-b:a".into(), br],
                "ALAC" => {
                    let mut v = vec!["-c:a".to_string(), "alac".into()];
                    v.extend(["-sample_fmt".to_string(), if depth >= 24 { "s32p".into() } else { "s16p".to_string() }]);
                    v
                }
                _ => {
                    let mut v = vec!["-c:a".to_string(), "flac".into()];
                    v.extend(["-sample_fmt".to_string(), if depth >= 24 { "s32".into() } else { "s16".to_string() }]);
                    v
                }
            };
            codec.extend(["-ar".to_string(), rate.to_string(), "-ac".to_string(), channels.to_string()]);
            let mut c = std::process::Command::new("ffmpeg");
            c.args(["-y", "-loglevel", "quiet", "-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "-"]);
            let art = std::path::Path::new(&tags.art);
            let use_art = !tags.art.is_empty() && art.is_file() && matches!(other, "MP3" | "AAC" | "ALAC" | "FLAC");
            if use_art { c.arg("-i").arg(art).args(["-map", "0:a", "-map", "1:v", "-c:v", "copy", "-disposition:v:0", "attached_pic"]); }
            c.args(&codec);
            if other == "MP3" { c.args(["-id3v2_version", "3"]); }
            if ext == "mp4" { c.args(["-movflags", "+frag_keyframe+empty_moov"]); }
            for (k, v) in tags.pairs() { if !v.is_empty() { c.arg("-metadata").arg(format!("{k}={v}")); } }
            let mut child = c.arg(&path).stdin(std::process::Stdio::piped()).spawn().map_err(|e| e.to_string())?;
            let stdin = child.stdin.take();
            Writer::Ffmpeg { child, stdin }
        }
    };
    Ok(Recording { writer, path: ps, frames: 0, bytes: 0, rng: 0x9E3779B97F4A7C15, err: [0.0; 2], channels, resamp: Resampler::new(48000, rate), tags })
}

fn write_frames(r: &mut Recording, data: &[Frame], depth: u16, dither: i32) {
    // sound for the files we write ourselves: mix down to mono if asked, then change the sample rate
    let data2: Vec<Frame>;
    let data_ref: &[Frame] = if matches!(r.writer, Writer::Ffmpeg { .. }) { data } else {
        let mut v = data.to_vec();
        if r.channels == 1 { for f in v.iter_mut() { let m = 0.5 * (f[0] + f[1]); *f = [m, m]; } }
        data2 = r.resamp.run(&v);
        &data2
    };
    let channels = r.channels as usize;
    match &mut r.writer {
        Writer::Wav(w) => {
            for f in data_ref {
                for c in 0..channels {
                    let x = f[c].clamp(-1.0, 1.0);
                    let _ = match depth {
                        32 => w.write_sample(x),
                        24 => w.write_sample((x * 8388607.0) as i32),
                        _ => {
                            // dithering: 0 none, 1 flat triangular noise (TPDF), 2 TPDF with first-order noise shaping
                            let mut v = x * 32767.0;
                            if dither > 0 {
                                r.rng = r.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                                let a = ((r.rng >> 40) as f32 / 16777216.0) - 0.5;
                                r.rng = r.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                                let b = ((r.rng >> 40) as f32 / 16777216.0) - 0.5;
                                v += a + b;
                                if dither == 2 { v -= r.err[c]; }
                            }
                            let q = v.round().clamp(-32768.0, 32767.0);
                            if dither == 2 { r.err[c] = q - v; }
                            w.write_sample(q as i32)
                        }
                    };
                }
            }
            r.bytes += data_ref.len() as u64 * channels as u64 * (depth.max(16) as u64 / 8);
        }
        Writer::Aiff { f, frames, bits, .. } => {
            for fr in data_ref {
                for c in 0..channels {
                    let x = fr[c].clamp(-1.0, 1.0);
                    if *bits == 24 { let v = ((x * 8388607.0) as i32).to_be_bytes(); let _ = f.write_all(&v[1..]); }
                    else { let v = (x * 32767.0) as i16; let _ = f.write_all(&v.to_be_bytes()); }
                }
            }
            *frames += data_ref.len() as u32;
            r.bytes += data_ref.len() as u64 * channels as u64 * (*bits as u64 / 8);
        }
        Writer::Ffmpeg { stdin, .. } => {
            if let Some(s) = stdin {
                let mut bytes = Vec::with_capacity(data_ref.len() * 8);
                for f in data_ref { bytes.extend_from_slice(&f[0].to_le_bytes()); bytes.extend_from_slice(&f[1].to_le_bytes()); }
                let _ = s.write_all(&bytes);
            }
            r.bytes += data_ref.len() as u64 * 4;
        }
    }
    r.frames += data.len() as u64;
}

fn close_recording(r: Recording) {
    match r.writer {
        Writer::Wav(w) => { let _ = w.finalize(); }
        Writer::Aiff { mut f, frames, bits, rate, channels } => {
            use std::io::{Seek, SeekFrom};
            let _ = f.flush();
            let bytes_per = (bits / 8) as u32 * channels as u32;
            let data_len = frames * bytes_per;
            let mut head: Vec<u8> = vec![];
            head.extend(b"FORM");
            head.extend((46 + data_len).to_be_bytes());
            head.extend(b"AIFF");
            head.extend(b"COMM");
            head.extend(18u32.to_be_bytes());
            head.extend(channels.to_be_bytes());
            head.extend(frames.to_be_bytes());
            head.extend(bits.to_be_bytes());
            head.extend(ieee_extended(rate));
            head.extend(b"SSND");
            head.extend((8 + data_len).to_be_bytes());
            head.extend([0u8; 8]);
            if let Ok(mut file) = f.into_inner() {
                let _ = file.seek(SeekFrom::Start(0));
                let _ = file.write_all(&head[..54.min(head.len())]);
            }
        }
        Writer::Ffmpeg { mut child, stdin } => {
            drop(stdin);
            let _ = child.wait();
        }
    }
}

/// Finishes a file: closes it, writes tags into files we wrote ourselves, and reports that the file is done.
fn end_file(sh: &Shared, block: u64, rec: Recording) {
    let path = rec.path.clone();
    let seconds = rec.frames as f64 / SR as f64;
    let tags = rec.tags.clone();
    let via_ffmpeg = matches!(rec.writer, Writer::Ffmpeg { .. });
    close_recording(rec);
    if !via_ffmpeg && !tags.is_empty() && ffmpeg_available() { let _ = crate::media::write_tags(std::path::Path::new(&path), &tags); }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    sh.events.lock().unwrap().push(FileEnd { block, path, seconds, size });
}

struct RecState {
    rec: Option<Recording>,
    counter: u32,
    silent_frames: u64,
    silent_state: bool,
    finished: bool,
    last_split_req: f64,
    elapsed_total: f64,
    file_started: bool,
}

// ---------------- graph ----------------

struct Rt {
    dsp: Dsp,
    rs: Option<RecState>,
    stream: Option<Stream>,
    stream2: Option<Stream>,
    device: String,
    // tf_tx must be dropped before sink so the transcription worker can finish
    tf_tx: Option<std::sync::mpsc::SyncSender<Vec<Frame>>>,
    sink: Option<Sink>,
    started: Option<Instant>,
}

// ---------------- streaming and transcription sinks (run through external programs) ----------------

/// A helper program that receives raw stereo float audio on its input (ffmpeg for streaming).
struct Sink {
    tx: std::sync::mpsc::SyncSender<Vec<f32>>,
    alive: Arc<AtomicBool>,
    pid: Arc<std::sync::atomic::AtomicI32>,
    handle: Option<std::thread::JoinHandle<()>>,
    note: Arc<Mutex<String>>,
}

impl Sink {
    fn ffmpeg(args: Vec<String>) -> Sink { Sink::ffmpeg_to(args, None) }

    fn ffmpeg_to(args: Vec<String>, cast: Option<Cast>) -> Sink {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(200);
        let alive = Arc::new(AtomicBool::new(true));
        let pid = Arc::new(std::sync::atomic::AtomicI32::new(0));
        let note = Arc::new(Mutex::new(String::new()));
        let (a, p, n) = (alive.clone(), pid.clone(), note.clone());
        let handle = std::thread::spawn(move || {
            let mut child = match std::process::Command::new("ffmpeg").args(&args).stdin(std::process::Stdio::piped()).stdout(if cast.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() }).stderr(std::process::Stdio::piped()).spawn() {
                Ok(c) => c,
                Err(_) => { *n.lock().unwrap() = "o ffmpeg não está instalado".into(); a.store(false, Ordering::Relaxed); return; }
            };
            p.store(child.id() as i32, Ordering::Relaxed);
            let mut stdin = child.stdin.take();
            let err = child.stderr.take();
            let n2 = n.clone();
            // Shoutcast: the encoded stream from ffmpeg is forwarded to the server by hand
            let fwd = match (cast, child.stdout.take()) {
                (Some(c), Some(out)) => { let n3 = n.clone(); let pid = child.id() as i32; Some(std::thread::spawn(move || { if let Err(e) = shoutcast_forward(c, out) { *n3.lock().unwrap() = e; unsafe { libc::kill(pid, libc::SIGTERM); } } })) }
                _ => None,
            };
            let t = std::thread::spawn(move || {
                if let Some(mut e) = err {
                    let mut s = String::new();
                    let _ = std::io::Read::read_to_string(&mut e, &mut s);
                    if let Some(l) = s.lines().rev().find(|l| !l.trim().is_empty()) { *n2.lock().unwrap() = l.chars().take(90).collect(); }
                }
            });
            while let Ok(chunk) = rx.recv() {
                if let Some(si) = stdin.as_mut() {
                    let mut bytes = Vec::with_capacity(chunk.len() * 4);
                    for f in &chunk { bytes.extend_from_slice(&f.to_le_bytes()); }
                    if si.write_all(&bytes).is_err() { break; }
                }
            }
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            let _ = t.join();
            if let Some(f) = fwd { let _ = f.join(); }
            a.store(false, Ordering::Relaxed);
        });
        Sink { tx, alive, pid, handle: Some(handle), note }
    }
    fn send(&self, f: &[Frame]) {
        let mut v = Vec::with_capacity(f.len() * 2);
        for x in f { v.push(x[0]); v.push(x[1]); }
        let _ = self.tx.try_send(v); // if the network is slow, audio is dropped instead of stalling everything
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        let pid = self.pid.load(Ordering::Relaxed);
        if pid > 0 { unsafe { libc::kill(pid, libc::SIGTERM); } }
        // replacing the sender closes the channel, which ends the writer thread
        let (dead_tx, _) = std::sync::mpsc::sync_channel(1);
        self.tx = dead_tx;
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
}

/// What a classic Shoutcast server needs to know (source protocol on port + 1).
pub struct Cast {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub headers: Vec<(String, String)>,
}

fn shoutcast_forward(c: Cast, mut out: std::process::ChildStdout) -> Result<(), String> {
    use std::io::Read;
    use std::net::ToSocketAddrs;
    let addr = (c.host.as_str(), c.port).to_socket_addrs().map_err(|_| format!("servidor “{}” não encontrado", c.host))?.next().ok_or("servidor não encontrado")?;
    let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| format!("sem conexão com o servidor: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    s.write_all(format!("{}\r\n", c.password).as_bytes()).map_err(|e| e.to_string())?;
    let mut resp = [0u8; 128];
    let n = s.read(&mut resp).map_err(|_| "o servidor não respondeu".to_string())?;
    let text = String::from_utf8_lossy(&resp[..n]).to_string();
    if !text.starts_with("OK") { return Err(format!("o servidor recusou a senha ({})", text.trim().chars().take(40).collect::<String>())); }
    let mut head = String::new();
    for (k, v) in &c.headers { head += &format!("{k}:{v}\r\n"); }
    head += "\r\n";
    s.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 4096];
    loop {
        let n = out.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 { return Ok(()); }
        s.write_all(&buf[..n]).map_err(|_| "o servidor fechou a conexão".to_string())?;
    }
}

/// Classic Shoutcast (version 1 source protocol; Shoutcast 2 servers accept it too, with ":#stream" after the password).
pub fn shoutcast_job(p: &crate::model::Params) -> Result<(Vec<String>, Cast), String> {
    let host = p.text("host", "");
    if host.trim().is_empty() { return Err("informe o endereço do servidor".into()); }
    let v2 = p.text("server", "") == "Shoutcast 2";
    let sid = p.text("stream_id", "");
    let password = if v2 && !sid.trim().is_empty() { format!("{}:#{}", p.text("password", ""), sid.trim()) } else { p.text("password", "") };
    let port = p.num("port", 8000.0) as u32 + 1;
    let br = p.num("bitrate", 128.0) as i32;
    let aac = p.text("format", "MP3") == "AAC";
    let mut a: Vec<String> = ["-loglevel", "error", "-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "-"].iter().map(|s| s.to_string()).collect();
    if aac { a.extend(["-c:a", "aac", "-b:a", &format!("{br}k"), "-f", "adts", "pipe:1"].iter().map(|s| s.to_string())); }
    else { a.extend(["-c:a", "libmp3lame", "-b:a", &format!("{br}k"), "-f", "mp3", "pipe:1"].iter().map(|s| s.to_string())); }
    let headers = vec![
        ("icy-name".to_string(), p.text("name", "")),
        ("icy-genre".to_string(), p.text("genre", "")),
        ("icy-url".to_string(), p.text("url", "")),
        ("icy-pub".to_string(), if p.num("public", 0.0) > 0.5 { "1".into() } else { "0".into() }),
        ("icy-br".to_string(), br.to_string()),
        ("content-type".to_string(), if aac { "audio/aacp".into() } else { "audio/mpeg".into() }),
    ];
    Ok((a, Cast { host: host.trim().to_string(), port: port as u16, password, headers }))
}

fn pct_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn broadcast_args(p: &crate::model::Params) -> Result<Vec<String>, String> {
    let host = p.text("host", "");
    if host.trim().is_empty() { return Err("informe o endereço do servidor".into()); }
    let mut mount = p.text("mount", "/stream");
    if !mount.starts_with('/') { mount.insert(0, '/'); }
    let url = format!("icecast://{}:{}@{}:{}{}", pct_encode(&p.text("user", "source")), pct_encode(&p.text("password", "")), host.trim(), p.num("port", 8000.0) as u32, mount);
    let br = format!("{}k", p.num("bitrate", 128.0) as i32);
    let aac = p.text("format", "MP3") == "AAC";
    let mut a: Vec<String> = ["-loglevel", "error", "-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "-"].iter().map(|s| s.to_string()).collect();
    let codec: Vec<&str> = if aac { vec!["-c:a", "aac", "-b:a", &br, "-content_type", "audio/aac", "-f", "adts"] } else { vec!["-c:a", "libmp3lame", "-b:a", &br, "-content_type", "audio/mpeg", "-f", "mp3"] };
    a.extend(codec.iter().map(|s| s.to_string()));
    for (k, v) in [("-ice_name", p.text("name", "")), ("-ice_genre", p.text("genre", "")), ("-ice_url", p.text("url", ""))] {
        if !v.is_empty() { a.push(k.into()); a.push(v); }
    }
    a.push("-ice_public".into());
    a.push(if p.num("public", 0.0) > 0.5 { "1".into() } else { "0".into() });
    a.push(url);
    Ok(a)
}

fn rtmp_args(p: &crate::model::Params) -> Result<Vec<String>, String> {
    let url = p.text("url", "");
    if url.trim().is_empty() { return Err("informe o endereço do servidor RTMP".into()); }
    let key = p.text("key", "");
    let dest = if key.is_empty() { url.trim().to_string() } else { format!("{}/{}", url.trim().trim_end_matches('/'), key) };
    // our quality levels (video kbit/s, audio kbit/s): Leve for slow uplinks, Padrão for most services, Alta for good connections
    let (vb, ab) = rtmp_quality(p.num("quality", 1.0) as i32);
    let mut a: Vec<String> = ["-loglevel", "error", "-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "-", "-f", "lavfi", "-i", "color=c=black:s=1280x720:r=30", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-g", "60"].iter().map(|s| s.to_string()).collect();
    a.extend(["-b:v", &format!("{vb}k"), "-maxrate", &format!("{vb}k"), "-bufsize", &format!("{}k", vb * 2), "-c:a", "aac", "-b:a", &format!("{ab}k"), "-f", "flv"].iter().map(|s| s.to_string()));
    a.push(dest);
    Ok(a)
}

/// Video and audio bit rates (kbit/s) of the three live-stream quality levels: Leve, Padrão and Alta.
pub fn rtmp_quality(level: i32) -> (u32, u32) {
    match level { 0 => (1000, 128), 2 => (6000, 192), _ => (3000, 160) }
}

/// Transcribe: every 10 s of audio goes through whisper.cpp (`whisper-cli`) and the text is appended to a file.
fn transcribe_worker(rx: std::sync::mpsc::Receiver<Vec<Frame>>, p: crate::model::Params, session: String, note: Arc<Mutex<String>>) {
    let dir = expand_home(&p.text("folder", &default_transcripts_dir()));
    let _ = std::fs::create_dir_all(&dir);
    let name = expand_tokens(&p.text("file_name", DEFAULT_TRANSCRIPT_NAME), &session, 1);
    let path = unique_path(&dir, &name, "txt");
    let tmp = std::env::temp_dir().join(format!("kalango-audio-transcribe-{}.wav", std::process::id()));
    let mut acc: Vec<Frame> = vec![];
    while let Ok(chunk) = rx.recv() {
        acc.extend(chunk);
        if acc.len() < 480_000 { continue; }
        // whisper wants 16 kHz mono
        let mono: Vec<f32> = acc.chunks(3).map(|c| 0.5 * (c[0][0] + c[0][1])).collect();
        acc.clear();
        let spec = hound::WavSpec { channels: 1, sample_rate: 16000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        if let Ok(mut w) = hound::WavWriter::create(&tmp, spec) {
            for x in &mono { let _ = w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16); }
            let _ = w.finalize();
        }
        let out = std::process::Command::new(p.text("command", "whisper-cli"))
            .args(["-m", &expand_home(&p.text("model", "")).to_string_lossy(), "-f", &tmp.to_string_lossy(), "-nt", "-np", "-l", &p.text("language", "auto")])
            .stderr(std::process::Stdio::null()).output();
        match out {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !text.is_empty() {
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) { let _ = writeln!(f, "{text}"); }
                    *note.lock().unwrap() = text.chars().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect();
                }
            }
            Err(_) => { *note.lock().unwrap() = "error: o whisper-cli não foi encontrado".into(); break; }
        }
    }
}

fn limit_frames(unit: &str, size: f64) -> (bool, f64) {
    // returns (is_time, amount in seconds or megabytes)
    if let Some(m) = unit.strip_prefix("time*") { (true, size * m.parse::<f64>().unwrap_or(60.0)) }
    else if let Some(m) = unit.strip_prefix("space*") { (false, size * m.parse::<f64>().unwrap_or(1.0)) }
    else { (true, size * 60.0) }
}

fn run(sh: Arc<Shared>, stop: Arc<AtomicBool>) {
    let mut rt: HashMap<u64, Rt> = HashMap::new();
    let mut bufs: HashMap<u64, Vec<Frame>> = HashMap::new();
    let mut hold: HashMap<u64, Meter> = HashMap::new();
    let chunk_dur = Duration::from_micros(10_000);
    let mut next = Instant::now();
    let mut load_avg = 0.0f32;
    while !stop.load(Ordering::Relaxed) {
        let work_start = Instant::now();
        let blocks = sh.blocks.lock().unwrap().clone();
        let session = sh.session_name.lock().unwrap().clone();
        rt.retain(|id, _| blocks.iter().any(|b| b.id == *id));

        // order: sources first, then follow the wires
        let order = topo(&blocks);
        bufs.clear();
        for id in order {
            let b = blocks.iter().find(|b| b.id == id).unwrap();
            let r = rt.entry(id).or_insert_with(|| Rt { dsp: Dsp::default(), rs: None, stream: None, stream2: None, device: String::new(), sink: None, started: None, tf_tx: None });
            // gather input
            let mut inp = vec![[0f32; 2]; CHUNK];
            let mut ups: Vec<(u64, Vec<Frame>)> = vec![];
            for up in blocks.iter().filter(|u| u.enabled && u.outs.contains(&id)) {
                if let Some(ub) = bufs.get(&up.id) {
                    for (o, i) in inp.iter_mut().zip(ub) { o[0] += i[0]; o[1] += i[1]; }
                    ups.push((up.id, ub.clone()));
                }
            }
            ups.sort_by_key(|u| u.0);
            let mut out = inp;
            if is_source(&b.kind) {
                out = vec![[0.0; 2]; CHUNK];
                if b.enabled {
                    if b.kind == "test_tone" {
                        r.dsp.tone(b.p.num("freq", 440.0) as f32, b.p.num("level", 0.25) as f32, &mut out);
                    } else {
                        let is_app = b.kind == "app_source";
                        let dev = if is_app { format!("app:{}", b.p.text("app", "Web Browser")) } else { b.p.text("device", "default") };
                        if r.stream.is_none() || r.device != dev {
                            r.stream = Some(if is_app { Stream::start_app(&b.p.text("app", "Web Browser")) } else { Stream::start(&dev, true) });
                            r.device = dev;
                        }
                        let st = r.stream.as_ref().unwrap();
                        {
                            let mut errs = sh.errors.lock().unwrap();
                            match st.error.lock().unwrap().clone() { Some(e) => { errs.insert(id, e); } None => { errs.remove(&id); } }
                        }
                        {
                            let mut q = st.queue.lock().unwrap();
                            for o in out.iter_mut() { *o = q.pop_front().unwrap_or([0.0; 2]); }
                        }
                        // Advanced: also listen to the default input, mixed in or kept on the left channel
                        if is_app && b.p.num("capture_inputs", 0.0) > 0.5 {
                            let st2 = r.stream2.get_or_insert_with(|| Stream::start("default", true));
                            let mut q2 = st2.queue.lock().unwrap();
                            let input: Vec<Frame> = (0..out.len()).map(|_| q2.pop_front().unwrap_or([0.0; 2])).collect();
                            out = combine_app_input(&out, &input, b.p.num("split_channels", 0.0) > 0.5);
                        } else { r.stream2 = None; }
                    }
                } else { r.stream = None; r.stream2 = None; }
            } else if b.enabled {
                match b.kind.as_str() {
                    "device_output" => {
                        let g = if b.p.num("muted", 0.0) > 0.5 { 0.0 } else { b.p.num("gain", 1.0) as f32 };
                        let dev = b.p.text("device", "default");
                        if r.stream.is_none() || r.device != dev {
                            r.stream = Some(Stream::start(&dev, false));
                            r.device = dev;
                        }
                        let st = r.stream.as_ref().unwrap();
                        if let Some(e) = st.error.lock().unwrap().clone() { sh.errors.lock().unwrap().insert(id, e); }
                        let mut q = st.queue.lock().unwrap();
                        for f in &out { q.push_back([f[0] * g, f[1] * g]); }
                        while q.len() > 9600 { q.pop_front(); }
                    }
                    "recorder" => record(&sh, b, r, &out, &session),
                    "mixer" => {
                        let tracks: Vec<Vec<Frame>> = ups.iter().take(5).map(|u| u.1.clone()).collect();
                        out = crate::dsp::mix_tracks(&tracks, &b.p, &mut r.dsp.fade_out);
                        if out.is_empty() { out = vec![[0f32; 2]; CHUNK]; }
                    }
                    "ducking" => {
                        let want = b.p.num("overlay", 0.0) as u64;
                        let oi = ups.iter().position(|u| u.0 == want).unwrap_or(0);
                        if let Some((oid, overlay)) = ups.get(oi) {
                            let mut others = vec![[0f32; 2]; CHUNK];
                            for (uid, ub) in &ups { if uid != oid { for (o, f) in others.iter_mut().zip(ub) { o[0] += f[0]; o[1] += f[1]; } } }
                            out = r.dsp.duck(overlay, &others, b.p.num("threshold", -30.0) as f32, b.p.num("ducked", 0.4) as f32);
                        }
                    }
                    "source_switch" => {
                        let i = b.p.num("switch", 0.0) as usize;
                        out = ups.get(i).map(|u| u.1.clone()).or_else(|| ups.first().map(|u| u.1.clone())).unwrap_or(vec![[0f32; 2]; CHUNK]);
                    }
                    "radio_stream" | "rtmp_stream" => {
                        if r.sink.as_ref().map(|s| !s.alive.load(Ordering::Relaxed)).unwrap_or(false) && r.started.map(|t| t.elapsed().as_secs() > 2).unwrap_or(false) {
                            // the program ended: keep its last message and try again after a few seconds
                            let note = r.sink.as_ref().map(|s| s.note.lock().unwrap().clone()).unwrap_or_default();
                            sh.rec.lock().unwrap().insert(id, RecStatus { state: format!("error: {}", if note.is_empty() { "a transmissão parou" } else { &note }), ..Default::default() });
                            if r.started.map(|t| t.elapsed().as_secs() > 8).unwrap_or(false) { r.sink = None; }
                        } else {
                            if r.sink.is_none() {
                                let shout = b.kind == "radio_stream" && b.p.text("server", "Icecast 2") != "Icecast 2";
                                if shout {
                                    match shoutcast_job(&b.p) {
                                        Ok((a, c)) => { r.sink = Some(Sink::ffmpeg_to(a, Some(c))); r.started = Some(Instant::now()); }
                                        Err(msg) => { sh.rec.lock().unwrap().insert(id, RecStatus { state: format!("error: {msg}"), ..Default::default() }); }
                                    }
                                } else {
                                let args = if b.kind == "radio_stream" { broadcast_args(&b.p) } else { rtmp_args(&b.p) };
                                match args {
                                    Ok(a) => { r.sink = Some(Sink::ffmpeg(a)); r.started = Some(Instant::now()); }
                                    Err(msg) => { sh.rec.lock().unwrap().insert(id, RecStatus { state: format!("error: {msg}"), ..Default::default() }); }
                                }
                                }
                            }
                            if let Some(s) = r.sink.as_ref() {
                                s.send(&out);
                                let secs = r.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
                                sh.rec.lock().unwrap().insert(id, RecStatus { seconds: secs, state: "streaming".into(), ..Default::default() });
                            }
                        }
                    }
                    "transcribe" => {
                        if r.sink.is_none() {
                            if b.p.text("model", "").trim().is_empty() {
                                sh.rec.lock().unwrap().insert(id, RecStatus { state: "error: escolha um arquivo de modelo do whisper (.bin)".into(), ..Default::default() });
                            } else {
                                let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<Frame>>(64);
                                let note = Arc::new(Mutex::new(String::new()));
                                let (p2, s2, n2) = (b.p.clone(), session.clone(), note.clone());
                                let handle = std::thread::spawn(move || transcribe_worker(rx, p2, s2, n2));
                                let (dtx, _) = std::sync::mpsc::sync_channel(1);
                                r.sink = Some(Sink { tx: dtx, alive: Arc::new(AtomicBool::new(true)), pid: Arc::new(std::sync::atomic::AtomicI32::new(0)), handle: Some(handle), note });
                                r.tf_tx = Some(tx);
                                r.started = Some(Instant::now());
                            }
                        }
                        if let (Some(tx), Some(s)) = (r.tf_tx.as_ref(), r.sink.as_ref()) {
                            let _ = tx.try_send(out.clone());
                            let note = s.note.lock().unwrap().clone();
                            let st = if note.starts_with("error") { note } else if note.is_empty() { "listening".to_string() } else { format!("… {note}") };
                            sh.rec.lock().unwrap().insert(id, RecStatus { seconds: r.started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0), state: st, ..Default::default() });
                        }
                    }
                    _ => {
                        r.dsp.process(b, &mut out);
                        if matches!(b.kind.as_str(), "time_shift" | "noise_reduction" | "voice_denoise") {
                            sh.rec.lock().unwrap().insert(id, RecStatus { seconds: r.dsp.aux, state: b.kind.clone(), ..Default::default() });
                        }
                        if matches!(b.kind.as_str(), "convolution" | "ladspa_plugin") {
                            sh.rec.lock().unwrap().insert(id, RecStatus { state: r.dsp.note.clone(), ..Default::default() });
                        }
                    }
                }
            } else if matches!(b.kind.as_str(), "radio_stream" | "rtmp_stream" | "transcribe") {
                r.tf_tx = None;
                r.sink = None;
            } else if b.kind == "recorder" {
                if let Some(rs) = r.rs.as_mut() { if let Some(rec) = rs.rec.take() { end_file(&sh, id, rec); } }
            }
            // meter
            let mut m = Meter::default();
            let mut sq = [0f32; 2];
            for f in &out { for c in 0..2 { m.peak[c] = m.peak[c].max(f[c].abs()); sq[c] += f[c] * f[c]; } }
            for c in 0..2 { m.rms[c] = (sq[c] / CHUNK as f32).sqrt(); }
            let _ = &mut hold;
            sh.meters.lock().unwrap().insert(id, m);
            bufs.insert(id, out);
        }
        let load = work_start.elapsed().as_secs_f32() / chunk_dur.as_secs_f32();
        if load > 1.0 { sh.overruns.fetch_add(1, Ordering::Relaxed); }
        load_avg = load_avg * 0.9 + load * 0.1;
        sh.load.store((load_avg * 1000.0) as u32, Ordering::Relaxed);
        // pace to real time
        next += chunk_dur;
        let now = Instant::now();
        if next > now { std::thread::sleep(next - now); } else if now - next > Duration::from_millis(200) { next = now; }
    }
    for (rid, r) in rt.iter_mut() {
        if let Some(rs) = r.rs.as_mut() { if let Some(rec) = rs.rec.take() { end_file(&sh, *rid, rec); } }
    }
}

/// "Include audio input": the input is added to the program's sound, or, with "Split between channels",
/// the input goes on the left and the program's sound on the right.
pub fn combine_app_input(app: &[Frame], input: &[Frame], split: bool) -> Vec<Frame> {
    app.iter().zip(input).map(|(a, i)| {
        if split { [0.5 * (i[0] + i[1]), 0.5 * (a[0] + a[1])] } else { [a[0] + i[0], a[1] + i[1]] }
    }).collect()
}

fn topo(blocks: &[Block]) -> Vec<u64> {
    let mut indeg: HashMap<u64, usize> = blocks.iter().map(|b| (b.id, 0)).collect();
    for b in blocks { for o in &b.outs { if let Some(d) = indeg.get_mut(o) { *d += 1; } } }
    let mut ready: Vec<u64> = blocks.iter().filter(|b| indeg[&b.id] == 0).map(|b| b.id).collect();
    let mut order = vec![];
    while let Some(id) = ready.pop() {
        order.push(id);
        if let Some(b) = blocks.iter().find(|b| b.id == id) {
            for o in &b.outs {
                if let Some(d) = indeg.get_mut(o) { *d -= 1; if *d == 0 { ready.push(*o); } }
            }
        }
    }
    order
}

/// Silence detection defaults: below -50 dB counts as silence (room noise of a quiet microphone sits around
/// -60 dB, speech rarely drops under -40 dB), and it must last 10 s so pauses in speech are not cut.
pub const SILENCE_DB: f64 = -50.0;
pub const SILENCE_SECONDS: f64 = 10.0;

fn record(sh: &Shared, b: &Block, r: &mut Rt, input: &[Frame], session: &str) {
    let p = &b.p;
    let rs = r.rs.get_or_insert_with(|| RecState {
        rec: None, counter: 0, silent_frames: 0, silent_state: false, finished: false,
        last_split_req: p.num("split_req", 0.0), elapsed_total: 0.0, file_started: false,
    });
    let status_key = b.id;
    let paused = p.num("pause", 0.0) > 0.5;
    let depth = p.num("depth", 16.0) as u16;
    let mut state = "recording";

    // split button
    let req = p.num("split_req", 0.0);
    if req != rs.last_split_req {
        rs.last_split_req = req;
        if let Some(rec) = rs.rec.take() { end_file(sh, b.id, rec); }
    }

    // silence handling
    let peak = input.iter().fold(0f32, |m, f| m.max(f[0].abs()).max(f[1].abs()));
    let thr = db_to_lin(p.num("silence_db", SILENCE_DB) as f32);
    let silence_time = p.num("silence_time", SILENCE_SECONDS).clamp(0.0, 99.9);
    let silence_on = p.num("silence_on", 0.0) > 0.5;
    if silence_on {
        if peak < thr { rs.silent_frames += CHUNK as u64; } else { rs.silent_frames = 0; }
        let needed = (silence_time * SR as f64) as u64;
        let long_silence = rs.silent_frames >= needed;
        match p.num("silence_action", 1.0) as i32 {
            1 => { rs.silent_state = long_silence; }
            2 => { if long_silence { if let Some(rec) = rs.rec.take() { end_file(sh, b.id, rec); } rs.silent_state = true; } else if peak >= thr { rs.silent_state = false; } }
            3 => { if long_silence && rs.file_started { if let Some(rec) = rs.rec.take() { end_file(sh, b.id, rec); } rs.finished = true; } }
            _ => {}
        }
    } else { rs.silent_state = false; rs.silent_frames = 0; }

    if rs.finished {
        state = "finished";
    } else if paused {
        state = "paused";
    } else if rs.silent_state {
        state = "silent";
    } else {
        if rs.rec.is_none() {
            rs.counter += 1;
            match open_recording(b, session, rs.counter, sh.session_rate.load(Ordering::Relaxed)) {
                Ok(rec) => { rs.rec = Some(rec); rs.file_started = true; }
                Err(e) => { sh.rec.lock().unwrap().insert(status_key, RecStatus { state: format!("error: {e}"), ..Default::default() }); return; }
            }
        }
        if let Some(rec) = rs.rec.as_mut() { write_frames(rec, input, depth, p.num("dither", 0.0) as i32); }
        // file limits
        let (mut split_now, mut stop_now) = (false, false);
        if let Some(rec) = rs.rec.as_ref() {
            let secs = rec.frames as f64 / SR as f64;
            let mb = rec.bytes as f64 / 1_048_576.0;
            if p.num("split_on", 0.0) > 0.5 {
                let (is_t, amt) = limit_frames(&p.text("split_unit", "space*1"), p.num("split_size", 5.0));
                split_now = if is_t { secs >= amt } else { mb >= amt };
            }
            rs.elapsed_total += CHUNK as f64 / SR as f64;
            if p.num("stop_on", 0.0) > 0.5 {
                let (is_t, amt) = limit_frames(&p.text("stop_unit", "time*3600"), p.num("stop_size", 1.0));
                // stops only once the total has gone past the limit, so a one-hour limit gives a full hour
                stop_now = if is_t { rs.elapsed_total > amt } else { mb > amt };
            }
        }
        if split_now || stop_now {
            if let Some(rec) = rs.rec.take() { end_file(sh, b.id, rec); }
            if stop_now { rs.finished = true; state = "finished"; }
        }
    }
    let (file, secs, bytes) = match rs.rec.as_ref() {
        Some(r) => (r.path.clone(), r.frames as f64 / SR as f64, r.bytes),
        None => (String::new(), 0.0, 0),
    };
    let mut g = sh.rec.lock().unwrap();
    let old = g.get(&status_key).cloned().unwrap_or_default();
    g.insert(status_key, RecStatus {
        file: if file.is_empty() { old.file } else { file },
        seconds: if rs.rec.is_some() { secs } else { old.seconds },
        bytes: if rs.rec.is_some() { bytes } else { old.bytes },
        state: state.to_string(),
    });
    let _ = lin_to_db(1.0);
}

#[cfg(test)]
mod tests {
    #[test]
    fn program_sound_and_input_are_mixed_or_split_between_channels() {
        let app = vec![[0.5f32, 0.3]; 4];
        let input = vec![[0.1f32, 0.1]; 4];
        let mixed = super::combine_app_input(&app, &input, false);
        assert!((mixed[0][0] - 0.6).abs() < 1e-6 && (mixed[0][1] - 0.4).abs() < 1e-6);
        let split = super::combine_app_input(&app, &input, true);
        assert!((split[0][0] - 0.1).abs() < 1e-6, "the input is on the left");
        assert!((split[0][1] - 0.4).abs() < 1e-6, "the program is on the right");
    }

    use super::*;
    use std::io::{BufRead, BufReader};

    /// A tiny stand-in for a Shoutcast server: checks the login, the headers and that audio arrives.
    #[test]
    fn shoutcast_login_headers_and_audio_reach_the_server() {
        if !ffmpeg_available() { return; }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server_port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut pw = String::new();
            r.read_line(&mut pw).unwrap();
            s.write_all(b"OK2\r\nicy-caps:11\r\n\r\n").unwrap();
            let mut headers = vec![];
            loop { let mut l = String::new(); r.read_line(&mut l).unwrap(); if l.trim().is_empty() { break; } headers.push(l.trim().to_string()); }
            let mut bytes = 0usize;
            let mut buf = [0u8; 4096];
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(2) {
                match std::io::Read::read(&mut r, &mut buf) { Ok(0) | Err(_) => break, Ok(n) => bytes += n }
            }
            (pw.trim().to_string(), headers, bytes)
        });
        let mut p = crate::model::default_params("radio_stream");
        p.set_text("server", "Shoutcast 2"); p.set_text("host", "127.0.0.1"); p.set("port", (server_port - 1) as f64);
        p.set_text("password", "secret"); p.set_text("stream_id", "3"); p.set_text("name", "My Radio"); p.set_text("genre", "Jazz");
        let (args, cast) = shoutcast_job(&p).unwrap();
        assert_eq!(cast.port, server_port);
        let sink = Sink::ffmpeg_to(args, Some(cast));
        let tone: Vec<Frame> = (0..480).map(|i| { let s = (i as f32 * 0.058).sin() * 0.3; [s, s] }).collect();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(3500) { sink.send(&tone); std::thread::sleep(Duration::from_millis(10)); }
        drop(sink);
        let (pw, headers, bytes) = server.join().unwrap();
        assert_eq!(pw, "secret:#3");
        assert!(headers.contains(&"icy-name:My Radio".to_string()), "{headers:?}");
        assert!(headers.contains(&"icy-genre:Jazz".to_string()));
        assert!(headers.contains(&"content-type:audio/mpeg".to_string()));
        assert!(bytes > 2000, "only {bytes} bytes of audio arrived");
    }

    #[test]
    fn shoutcast_needs_an_address_and_a_wrong_login_is_reported() {
        let p = crate::model::default_params("radio_stream");
        assert!(shoutcast_job(&p).is_err());
        assert!(broadcast_args(&p).is_err());
        let mut p = p;
        p.set_text("host", "radio.example.com"); p.set_text("password", "pw");
        let (_, c) = shoutcast_job(&p).unwrap();
        assert_eq!(c.password, "pw");
        assert_eq!(c.port, 8001);
    }

    #[test]
    fn default_folders_live_under_home_and_carry_our_name() {
        let rec = default_recordings_dir();
        assert!(rec.ends_with("/Kalango Audio"), "{rec}");
        assert!(!expand_home(&rec).to_string_lossy().contains('~'));
        assert!(default_transcripts_dir().ends_with("/Kalango Audio"));
        assert!(!documents_dir().is_empty());
    }

    #[test]
    fn live_stream_quality_levels_grow() {
        let (a, b, c) = (rtmp_quality(0), rtmp_quality(1), rtmp_quality(2));
        assert_eq!((a.0, b.0, c.0), (1000, 3000, 6000));
        assert!(a.1 <= b.1 && b.1 <= c.1);
    }
}
