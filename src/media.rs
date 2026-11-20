//! Tags and playback for recordings (through ffprobe / ffmpeg) and the little player of the Recordings window.
use crate::i18n::tr;
use crate::audio::Stream;
use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub comment: String,
    pub year: String,
    pub track: String,
    pub track_count: String,
    pub disc: String,
    pub disc_count: String,
    pub compilation: bool,
    /// a picture to attach (only used when writing)
    pub art: String,
}

impl Tags {
    pub fn is_empty(&self) -> bool {
        *self == Tags::default()
    }

    /// The pairs that go to ffmpeg's -metadata (empty values clear a tag).
    pub fn pairs(&self) -> Vec<(String, String)> {
        let n = |a: &str, b: &str| if a.is_empty() { String::new() } else if b.is_empty() { a.to_string() } else { format!("{a}/{b}") };
        vec![
            ("title".into(), self.title.clone()),
            ("artist".into(), self.artist.clone()),
            ("album".into(), self.album.clone()),
            ("genre".into(), self.genre.clone()),
            ("comment".into(), self.comment.clone()),
            ("date".into(), self.year.clone()),
            ("track".into(), n(&self.track, &self.track_count)),
            ("disc".into(), n(&self.disc, &self.disc_count)),
            ("compilation".into(), if self.compilation { "1".into() } else { String::new() }),
        ]
    }
}

pub fn have(tool: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file())).unwrap_or(false)
}

fn split_pair(v: &str) -> (String, String) {
    match v.split_once('/') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (v.trim().to_string(), String::new()),
    }
}

pub fn read_tags(path: &Path) -> Tags {
    let mut t = Tags::default();
    let Ok(o) = Command::new("ffprobe").args(["-v", "error", "-show_entries", "format_tags", "-of", "json"]).arg(path).stderr(Stdio::null()).output() else { return t };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) else { return t };
    let Some(map) = v["format"]["tags"].as_object() else { return t };
    let get = |k: &str| map.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).and_then(|(_, v)| v.as_str()).unwrap_or("").to_string();
    t.title = get("title");
    t.artist = get("artist");
    t.album = get("album");
    t.genre = get("genre");
    t.comment = get("comment");
    t.year = get("date").chars().take(4).collect();
    (t.track, t.track_count) = split_pair(&get("track"));
    (t.disc, t.disc_count) = split_pair(&get("disc"));
    t.compilation = matches!(get("compilation").as_str(), "1" | "true");
    t
}

pub fn duration(path: &Path) -> Option<f64> {
    let o = Command::new("ffprobe").args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"]).arg(path).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&o.stdout).trim().parse().ok()
}

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
}

/// Writes tags (and a picture, for formats that can carry one) by copying the audio into a temporary file
/// and moving that over the file that was there.
pub fn write_tags(path: &Path, tags: &Tags) -> Result<(), String> {
    if !have("ffmpeg") { return Err(tr("ffmpeg is needed to write the tags").into()); }
    let ext = ext_of(path);
    let tmp = path.with_extension(format!("tagtmp.{ext}"));
    let art = Path::new(&tags.art);
    let use_art = !tags.art.is_empty() && art.is_file() && matches!(ext.as_str(), "mp3" | "m4a" | "mp4" | "flac");
    let mut c = Command::new("ffmpeg");
    c.args(["-y", "-loglevel", "error", "-i"]).arg(path);
    if use_art { c.arg("-i").arg(art).args(["-map", "0:a", "-map", "1:v", "-disposition:v:0", "attached_pic"]); } else { c.args(["-map", "0"]); }
    c.args(["-c", "copy"]);
    if ext == "mp3" { c.args(["-id3v2_version", "3"]); }
    for (k, v) in tags.pairs() { c.arg("-metadata").arg(format!("{k}={v}")); }
    c.arg(&tmp);
    let out = c.stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(tr("ffmpeg failed")).to_string();
        trash_quietly(&tmp);
        return Err(msg);
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Moves a file to the trash (never deletes it for good). Returns whether it worked.
pub fn trash(path: &Path) -> bool {
    Command::new("gio").arg("trash").arg(path).status().map(|s| s.success()).unwrap_or(false)
}

fn trash_quietly(path: &Path) {
    let _ = trash(path);
}

// ---------------- player ----------------

/// Plays a file through the default sound output: ffmpeg decodes, the sound card stream plays.
pub struct Player {
    stream: Option<Stream>,
    stop: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pushed: Arc<AtomicU64>,
    eof: Arc<AtomicBool>,
    pid: Arc<AtomicI32>,
    start: f64,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Player {
    pub fn start(path: &Path, from: f64) -> Player {
        let stream = Stream::start("default", false);
        let queue = stream.queue.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let pushed = Arc::new(AtomicU64::new(0));
        let eof = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(AtomicI32::new(0));
        let (s, pa, pu, e, pi, p) = (stop.clone(), paused.clone(), pushed.clone(), eof.clone(), pid.clone(), path.to_path_buf());
        let handle = std::thread::spawn(move || {
            let child = Command::new("ffmpeg").args(["-loglevel", "quiet", "-ss", &format!("{from}"), "-i"]).arg(&p).args(["-f", "f32le", "-ar", "48000", "-ac", "2", "-"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
            let Ok(mut child) = child else { e.store(true, Ordering::Relaxed); return };
            pi.store(child.id() as i32, Ordering::Relaxed);
            let mut out = child.stdout.take().unwrap();
            let mut buf = vec![0u8; 480 * 8];
            while !s.load(Ordering::Relaxed) {
                if pa.load(Ordering::Relaxed) || queue.lock().unwrap().len() > 24000 { std::thread::sleep(std::time::Duration::from_millis(5)); continue; }
                match out.read_exact(&mut buf) {
                    Ok(()) => {
                        let mut g = queue.lock().unwrap();
                        let mut dq: VecDeque<[f32; 2]> = VecDeque::new();
                        for c in buf.chunks_exact(8) { dq.push_back([f32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_le_bytes([c[4], c[5], c[6], c[7]])]); }
                        pu.fetch_add(dq.len() as u64, Ordering::Relaxed);
                        g.extend(dq);
                    }
                    Err(_) => { e.store(true, Ordering::Relaxed); break; }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        });
        Player { stream: Some(stream), stop, paused, pushed, eof, pid, start: from, handle: Some(handle) }
    }

    /// Where playback is, in seconds.
    pub fn position(&self) -> f64 {
        let queued = self.stream.as_ref().map(|s| s.queue.lock().unwrap().len()).unwrap_or(0) as u64;
        self.start + self.pushed.load(Ordering::Relaxed).saturating_sub(queued) as f64 / 48000.0
    }

    pub fn finished(&self) -> bool {
        self.eof.load(Ordering::Relaxed) && self.stream.as_ref().map(|s| s.queue.lock().unwrap().is_empty()).unwrap_or(true)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let pid = self.pid.load(Ordering::Relaxed);
        if pid > 0 { unsafe { libc::kill(pid, libc::SIGTERM); } }
        if let Some(h) = self.handle.take() { let _ = h.join(); }
        self.stream = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_wav() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("kalango-audio-media-{}.wav", std::process::id()));
        let spec = hound::WavSpec { channels: 2, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&p, spec).unwrap();
        for i in 0..48000 { let s = ((i as f32 * 0.05).sin() * 8000.0) as i16; w.write_sample(s).unwrap(); w.write_sample(s).unwrap(); }
        w.finalize().unwrap();
        p
    }

    #[test]
    fn tags_are_written_and_read_back() {
        if !have("ffmpeg") || !have("ffprobe") { return; }
        let p = test_wav();
        let t = Tags { title: "Hello".into(), artist: "Me".into(), album: "Album".into(), genre: "Jazz".into(), comment: "note".into(), year: "2024".into(), track: "3".into(), track_count: "12".into(), disc: "1".into(), disc_count: "2".into(), compilation: true, art: String::new() };
        write_tags(&p, &t).unwrap();
        let back = read_tags(&p);
        assert_eq!(back.title, "Hello");
        assert_eq!(back.artist, "Me");
        assert_eq!(back.album, "Album");
        assert_eq!(back.genre, "Jazz");
        assert_eq!(back.year, "2024");
        assert_eq!((back.track.as_str(), back.track_count.as_str()), ("3", "12"));
        assert!(duration(&p).unwrap() > 0.99);
        // clearing a tag works too
        let mut t2 = back.clone();
        t2.title.clear();
        write_tags(&p, &t2).unwrap();
        assert_eq!(read_tags(&p).title, "");
        // the audio itself is untouched
        assert!(duration(&p).unwrap() > 0.99);
    }

    #[test]
    fn tag_pairs_and_numbers() {
        let t = Tags { track: "2".into(), track_count: "9".into(), disc: "1".into(), ..Default::default() };
        let pairs = t.pairs();
        assert!(pairs.contains(&("track".into(), "2/9".into())));
        assert!(pairs.contains(&("disc".into(), "1".into())));
        assert!(Tags::default().is_empty());
    }
}
