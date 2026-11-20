//! Sound card access through the system's ALSA library, loaded at run time (no build-time dependency).
use crate::i18n::{tr, trf};
use crate::dsp::Frame;
use libloading::Library;
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

type Open = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type SetParams = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type Io = unsafe extern "C" fn(*mut c_void, *mut c_void, c_ulong) -> c_long;
type IoW = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type Recover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void) -> c_int;

struct Alsa {
    _lib: Library,
    open: Open,
    set_params: SetParams,
    readi: Io,
    writei: IoW,
    recover: Recover,
    close: Close,
}

fn alsa() -> Option<&'static Alsa> {
    static A: OnceLock<Option<Alsa>> = OnceLock::new();
    A.get_or_init(|| unsafe {
        let lib = Library::new("libasound.so.2").ok()?;
        let open = *lib.get::<Open>(b"snd_pcm_open\0").ok()?;
        let set_params = *lib.get::<SetParams>(b"snd_pcm_set_params\0").ok()?;
        let readi = *lib.get::<Io>(b"snd_pcm_readi\0").ok()?;
        let writei = *lib.get::<IoW>(b"snd_pcm_writei\0").ok()?;
        let recover = *lib.get::<Recover>(b"snd_pcm_recover\0").ok()?;
        let close = *lib.get::<Close>(b"snd_pcm_close\0").ok()?;
        Some(Alsa { _lib: lib, open, set_params, readi, writei, recover, close })
    })
    .as_ref()
}

struct Pcm(*mut c_void);
unsafe impl Send for Pcm {}

const FORMAT_FLOAT_LE: c_int = 14;
const ACCESS_RW_INTERLEAVED: c_int = 3;
pub const CHUNK: usize = 480; // 10 ms at 48 kHz

fn open(name: &str, capture: bool) -> Result<Pcm, String> {
    let a = alsa().ok_or_else(|| tr("The system’s sound library (ALSA) was not found").to_string())?;
    let cname = CString::new(name).map_err(|_| tr("Invalid device name").to_string())?;
    let mut h: *mut c_void = std::ptr::null_mut();
    unsafe {
        let r = (a.open)(&mut h, cname.as_ptr(), if capture { 1 } else { 0 }, 0);
        if r < 0 { return Err(trf("Could not open “{name}”", &[("name", name.to_string())])); }
        let r = (a.set_params)(h, FORMAT_FLOAT_LE, ACCESS_RW_INTERLEAVED, 2, 48000, 1, 80_000);
        if r < 0 { (a.close)(h); return Err(trf("“{name}” does not take stereo audio", &[("name", name.to_string())])); }
    }
    Ok(Pcm(h))
}

pub type Queue = Arc<Mutex<VecDeque<Frame>>>;

/// A running capture or playback thread feeding / draining a queue of stereo frames.
pub struct Stream {
    pub queue: Queue,
    stop: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Stream {
    pub fn start(device: &str, capture: bool) -> Stream {
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (q, s, e, dev) = (queue.clone(), stop.clone(), error.clone(), device.to_string());
        let handle = std::thread::spawn(move || {
            let pcm = match open(&dev, capture) {
                Ok(p) => p,
                Err(m) => { *e.lock().unwrap() = Some(m); return; }
            };
            let a = alsa().unwrap();
            let mut buf = vec![0f32; CHUNK * 2];
            while !s.load(Ordering::Relaxed) {
                unsafe {
                    if capture {
                        let n = (a.readi)(pcm.0, buf.as_mut_ptr() as *mut c_void, CHUNK as c_ulong);
                        if n < 0 { (a.recover)(pcm.0, n as c_int, 1); continue; }
                        let mut g = q.lock().unwrap();
                        for i in 0..n as usize { g.push_back([buf[2 * i], buf[2 * i + 1]]); }
                        while g.len() > 48000 { g.pop_front(); }
                    } else {
                        {
                            let mut g = q.lock().unwrap();
                            for i in 0..CHUNK {
                                let f = g.pop_front().unwrap_or([0.0, 0.0]);
                                buf[2 * i] = f[0].clamp(-1.0, 1.0);
                                buf[2 * i + 1] = f[1].clamp(-1.0, 1.0);
                            }
                        }
                        let n = (a.writei)(pcm.0, buf.as_ptr() as *const c_void, CHUNK as c_ulong);
                        if n < 0 { (a.recover)(pcm.0, n as c_int, 1); }
                    }
                }
            }
            unsafe { (a.close)(pcm.0); }
        });
        Stream { queue, stop, error, handle: Some(handle) }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
}

/// Devices the person can pick: the system default plus each sound card.
pub fn list_devices() -> Vec<(String, String)> {
    let mut v = vec![("default".to_string(), tr("System default").to_string())];
    if let Ok(t) = std::fs::read_to_string("/proc/asound/cards") {
        for line in t.lines() {
            let l = line.trim_start();
            if let Some((num, rest)) = l.split_once(' ') {
                if num.parse::<u32>().is_ok() {
                    if let (Some(a), Some(b)) = (rest.find('['), rest.find(']')) {
                        let short = rest[a + 1..b].trim();
                        let long = rest.split(" - ").nth(1).unwrap_or(short).trim();
                        v.push((format!("plughw:{num},0"), format!("{long} ({short})")));
                    }
                }
            }
        }
    }
    v
}

pub fn device_label(id: &str) -> String {
    list_devices().into_iter().find(|d| d.0 == id).map(|d| d.1).unwrap_or_else(|| id.to_string())
}

// ---------------- per-application capture (PipeWire) ----------------

#[derive(Clone, Debug)]
pub struct AppInfo {
    pub name: String,
    pub binary: String,
    pub serial: String,
    pub node: String,
    pub pid: i32,
    pub icon: String,
}

/// Audio-playing programs right now (PipeWire output streams), found with `pw-dump`.
pub fn list_apps() -> Vec<AppInfo> {
    let out = match std::process::Command::new("pw-dump").stderr(std::process::Stdio::null()).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    let v: serde_json::Value = match serde_json::from_slice(&out.stdout) { Ok(v) => v, Err(_) => return vec![] };
    let mut apps = vec![];
    for o in v.as_array().cloned().unwrap_or_default() {
        let p = &o["info"]["props"];
        if p["media.class"].as_str() != Some("Stream/Output/Audio") { continue; }
        let node = p["node.name"].as_str().unwrap_or("").to_string();
        // never offer our own playback stream
        if node.contains("kalango-audio") || node.contains("kalango_audio") { continue; }
        let name = p["application.name"].as_str().unwrap_or(&node).to_string();
        let serial = match &p["object.serial"] { serde_json::Value::Number(n) => n.to_string(), serde_json::Value::String(s) => s.clone(), _ => continue };
        let pid = p["application.process.id"].as_i64().or_else(|| p["application.process.id"].as_str().and_then(|s| s.parse().ok())).unwrap_or(0) as i32;
        let icon = p["application.icon_name"].as_str().unwrap_or("").to_string();
        apps.push(AppInfo { name, binary: p["application.process.binary"].as_str().unwrap_or("").to_string(), serial, node, pid, icon });
    }
    apps
}

const BROWSERS: [&str; 12] = ["firefox", "chrom", "brave", "vivaldi", "opera", "edge", "epiphany", "librewolf", "zen", "safari", "waterfox", "falkon"];

const CALL_APPS: [&str; 10] = ["zoom", "skype", "teams", "discord", "telegram", "signal", "slack", "webex", "jitsi", "element"];

/// One installed program, read from its desktop entry.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub bin: String,
    pub icon: String,
    pub wm_class: String,
}

/// Reads the desktop entries in the given folders (the way programs are listed in an applications menu).
pub fn installed_apps_from(dirs: &[PathBuf]) -> Vec<DesktopApp> {
    let mut v: Vec<DesktopApp> = vec![];
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x != "desktop").unwrap_or(true) { continue; }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let (mut in_entry, mut name, mut exec, mut icon, mut wm) = (false, String::new(), String::new(), String::new(), String::new());
            let (mut hidden, mut is_app) = (false, true);
            for l in text.lines() {
                let l = l.trim();
                if l.starts_with('[') { in_entry = l == "[Desktop Entry]"; continue; }
                if !in_entry { continue; }
                if let Some((k, val)) = l.split_once('=') {
                    match k {
                        "Name" => name = val.to_string(),
                        "Exec" => exec = val.to_string(),
                        "Icon" => icon = val.to_string(),
                        "StartupWMClass" => wm = val.to_string(),
                        "NoDisplay" | "Hidden" => { if val == "true" { hidden = true; } }
                        "Type" => is_app = val == "Application",
                        _ => {}
                    }
                }
            }
            if hidden || !is_app || name.is_empty() || exec.is_empty() { continue; }
            // the program itself: first word of the command, without a path or a wrapper like env
            let words: Vec<&str> = exec.split_whitespace().filter(|w| !w.starts_with('%') && !w.contains('=')).collect();
            let first = words.iter().find(|w| **w != "env" && !w.starts_with('-')).copied().unwrap_or("");
            let bin = first.trim_matches('"').rsplit('/').next().unwrap_or("").to_string();
            let id = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if !v.iter().any(|x| x.id == id) { v.push(DesktopApp { id, name, bin, icon, wm_class: wm }); }
        }
    }
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

pub fn app_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut v = vec![PathBuf::from(&home).join(".local/share/applications"), PathBuf::from(&home).join(".local/share/flatpak/exports/share/applications")];
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data.split(':').filter(|d| !d.is_empty()) { v.push(PathBuf::from(d).join("applications")); }
    v.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    v.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    v
}

static INSTALLED: OnceLock<Vec<DesktopApp>> = OnceLock::new();

/// Every program installed on this computer that shows up in the applications menu.
pub fn installed_apps() -> &'static [DesktopApp] { INSTALLED.get_or_init(|| installed_apps_from(&app_dirs())) }

/// The program a block listens to, in words: "desktop:firefox" -> "Firefox".
pub fn app_label(want: &str) -> String {
    match want {
        "Web Browser" => tr("Browser (any)").into(),
        "Any Application" => tr("Any program").into(),
        "Call App" => tr("Video call (Zoom, Skype, Teams…)").into(),
        w => {
            if let Some(id) = w.strip_prefix("desktop:") { return installed_apps().iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| id.to_string()); }
            if let Some(n) = w.strip_prefix("name:") { let mut c = n.chars(); return c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default(); }
            w.to_string()
        }
    }
}

/// Does a running stream belong to the app the block asks for?
pub fn app_matches(want: &str, a: &AppInfo) -> bool { app_matches_in(want, a, installed_apps()) }

pub fn app_matches_in(want: &str, a: &AppInfo, installed: &[DesktopApp]) -> bool {
    let hay = format!("{} {} {}", a.name, a.binary, a.node).to_lowercase();
    match want {
        "Any Application" => true,
        "Web Browser" => BROWSERS.iter().any(|b| hay.contains(b)),
        "Call App" => CALL_APPS.iter().any(|b| hay.contains(b)),
        w => {
            if let Some(n) = w.strip_prefix("name:") { return hay.contains(&n.to_lowercase()); }
            if let Some(id) = w.strip_prefix("desktop:") {
                let Some(d) = installed.iter().find(|d| d.id == id) else { return hay.contains(&id.to_lowercase()) };
                let (n, b) = (a.name.to_lowercase(), a.binary.to_lowercase());
                return n == d.name.to_lowercase()
                    || (!d.bin.is_empty() && (b == d.bin.to_lowercase() || a.node.to_lowercase().contains(&d.bin.to_lowercase())))
                    || (!d.icon.is_empty() && a.icon == d.icon)
                    || (!d.wm_class.is_empty() && n == d.wm_class.to_lowercase());
            }
            a.name == w || a.binary == w
        }
    }
}

/// Opens the chosen program (with a web address, if given): the "Abrir um endereço" option.
pub fn launch_app(want: &str, url: &str) {
    let url = url.trim();
    let program = want.strip_prefix("desktop:").and_then(|id| installed_apps().iter().find(|d| d.id == id)).map(|d| d.bin.clone());
    let mut c = match (program, url.is_empty()) {
        (Some(bin), false) if !bin.is_empty() => { let mut c = std::process::Command::new(bin); c.arg(url); c }
        (Some(bin), true) if !bin.is_empty() => std::process::Command::new(bin),
        (_, false) => { let mut c = std::process::Command::new("xdg-open"); c.arg(url); c }
        _ => return,
    };
    let _ = c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, bin: &str, icon: &str) -> AppInfo { AppInfo { name: name.into(), binary: bin.into(), serial: "1".into(), node: format!("{bin}-node"), pid: 1, icon: icon.into() } }

    #[test]
    fn desktop_entries_are_read_into_a_program_list() {
        let dir = std::env::temp_dir().join(format!("kalango-audio-apps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("spotify.desktop"), "[Desktop Entry]\nType=Application\nName=Spotify\nExec=env FOO=1 /opt/spotify/spotify %U\nIcon=spotify-client\n").unwrap();
        std::fs::write(dir.join("hidden.desktop"), "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n").unwrap();
        std::fs::write(dir.join("link.desktop"), "[Desktop Entry]\nType=Link\nName=A link\nExec=x\n").unwrap();
        std::fs::write(dir.join("vlc.desktop"), "[Desktop Entry]\nType=Application\nName=VLC media player\nExec=/usr/bin/vlc --started-from-file %U\nIcon=vlc\n[Desktop Action Play]\nName=Other\nExec=other\n").unwrap();
        let apps = installed_apps_from(&[dir.clone(), PathBuf::from("/not/here")]);
        assert_eq!(apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Spotify", "VLC media player"]);
        assert_eq!(apps[0].bin, "spotify");
        assert_eq!(apps[1].bin, "vlc");
        assert_eq!(apps[0].id, "spotify");
    }

    #[test]
    fn programs_are_matched_the_way_the_block_asks() {
        let apps = vec![
            DesktopApp { id: "spotify".into(), name: "Spotify".into(), bin: "spotify".into(), icon: "spotify-client".into(), wm_class: String::new() },
            DesktopApp { id: "org.videolan.VLC".into(), name: "VLC media player".into(), bin: "vlc".into(), icon: "vlc".into(), wm_class: String::new() },
        ];
        let (sp, vlc, ff, zoom) = (info("spotify", "spotify", "spotify-client"), info("VLC media player", "vlc", "vlc"), info("Firefox", "firefox", ""), info("ZOOM VoiceEngine", "zoom", ""));
        assert!(app_matches_in("desktop:spotify", &sp, &apps));
        assert!(!app_matches_in("desktop:spotify", &vlc, &apps));
        assert!(app_matches_in("desktop:org.videolan.VLC", &vlc, &apps));
        assert!(app_matches_in("Web Browser", &ff, &apps) && !app_matches_in("Web Browser", &sp, &apps));
        assert!(app_matches_in("Call App", &zoom, &apps) && !app_matches_in("Call App", &ff, &apps));
        assert!(app_matches_in("name:zoom", &zoom, &apps));
        assert!(app_matches_in("Any Application", &sp, &apps));
        assert!(app_matches_in("Firefox", &ff, &apps), "older saved choices (a plain name) still work");
        assert_eq!(app_label("name:spotify"), "Spotify");
        assert_eq!(app_label("Call App"), "Video call (Zoom, Skype, Teams…)");
    }
}

struct Reader {
    queue: Queue,
    pid: Arc<std::sync::atomic::AtomicI32>,
    alive: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Reader {
    fn start(serial: &str) -> Reader {
        use std::io::Read;
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let pid = Arc::new(std::sync::atomic::AtomicI32::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let (q, p, a, serial) = (queue.clone(), pid.clone(), alive.clone(), serial.to_string());
        let handle = std::thread::spawn(move || {
            let child = std::process::Command::new("pw-record")
                .args(["--rate", "48000", "--channels", "2", "--format", "f32", "--latency", "20ms", "--target", &serial, "-"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn();
            let mut child = match child { Ok(c) => c, Err(_) => { a.store(false, Ordering::Relaxed); return; } };
            p.store(child.id() as i32, Ordering::Relaxed);
            if let Some(mut out) = child.stdout.take() {
                let mut buf = [0u8; 8 * 240];
                loop {
                    match out.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut g = q.lock().unwrap();
                            for c in buf[..n - n % 8].chunks_exact(8) {
                                g.push_back([f32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_le_bytes([c[4], c[5], c[6], c[7]])]);
                            }
                            while g.len() > 24000 { g.pop_front(); }
                        }
                    }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            a.store(false, Ordering::Relaxed);
        });
        Reader { queue, pid, alive, handle: Some(handle) }
    }
    fn stop(&mut self) {
        let pid = self.pid.load(Ordering::Relaxed);
        if pid > 0 { unsafe { libc::kill(pid, libc::SIGTERM); } }
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
}

impl Stream {
    /// Capture what one program (or every browser, or everything) is playing, mixed to one stereo queue.
    pub fn start_app(want: &str) -> Stream {
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let (q, s, e, want) = (queue.clone(), stop.clone(), error.clone(), want.to_string());
        let handle = std::thread::spawn(move || {
            let mut readers: Vec<(String, Reader)> = vec![];
            let mut last_scan = std::time::Instant::now() - std::time::Duration::from_secs(5);
            let mut next = std::time::Instant::now();
            while !s.load(Ordering::Relaxed) {
                if last_scan.elapsed().as_millis() > 1000 {
                    last_scan = std::time::Instant::now();
                    let found: Vec<AppInfo> = list_apps().into_iter().filter(|a| app_matches(&want, a)).collect();
                    readers.retain_mut(|(ser, r)| {
                        let keep = r.alive.load(Ordering::Relaxed) && found.iter().any(|a| &a.serial == ser);
                        if !keep { r.stop(); }
                        keep
                    });
                    for a in &found {
                        if !readers.iter().any(|(ser, _)| *ser == a.serial) { readers.push((a.serial.clone(), Reader::start(&a.serial))); }
                    }
                    *e.lock().unwrap() = if readers.is_empty() { Some(trf("Waiting for “{app}” to play some sound", &[("app", app_label(&want))])) } else { None };
                }
                // mix 10 ms from every stream into the output queue
                let mut mix = vec![[0f32; 2]; CHUNK];
                for (_, r) in readers.iter() {
                    let mut g = r.queue.lock().unwrap();
                    for m in mix.iter_mut() {
                        if let Some(f) = g.pop_front() { m[0] += f[0]; m[1] += f[1]; } else { break; }
                    }
                }
                {
                    let mut g = q.lock().unwrap();
                    g.extend(mix);
                    while g.len() > 48000 { g.pop_front(); }
                }
                next += std::time::Duration::from_millis(10);
                let now = std::time::Instant::now();
                if next > now { std::thread::sleep(next - now); } else { next = now; }
            }
            for (_, r) in readers.iter_mut() { r.stop(); }
        });
        Stream { queue, stop, error, handle: Some(handle) }
    }
}
