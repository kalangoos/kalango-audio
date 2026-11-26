//! Voice isolation through audio.cpp: the program-wide server (one for the whole app, started on first use and
//! stopped when the app ends or after a while without users), what the person is told when something is missing, and
//! the offline action on a recording.
//!
//! audio.cpp is the backend (`crates/kalango-audiocpp` controls its `audiocpp_server` process); the Rust port in
//! `crates/kalango-isolar-voz` stays as the parity reference and is not used here. Everything is local: the server
//! listens on 127.0.0.1 only, the sound never leaves the computer, and nothing is downloaded by this module.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kalango_audiocpp::discovery;
use kalango_audiocpp::live::LiveState;
use kalango_audiocpp::offline;
use kalango_audiocpp::service::{LiveLimit, Service, ServiceConfig, State};
use kalango_audiocpp::{Backend, Config, SAMPLE_RATE};

use crate::i18n::{tr, trf};

/// What is not installed: the place where each missing piece is expected.
#[derive(Debug, Clone, PartialEq)]
pub struct Missing {
    pub server: Option<PathBuf>,
    pub model: Option<PathBuf>,
}

impl Missing {
    /// What the person reads: what is missing and where to put it. Nothing is downloaded for them.
    pub fn message(&self) -> String {
        let mut lines = Vec::new();
        if let Some(p) = &self.server {
            lines.push(trf("The audio.cpp server program was not found. Put it at {path}, or set KALANGO_AUDIOCPP_SERVER to its location.", &[("path", shown(p))]));
        }
        if let Some(p) = &self.model {
            lines.push(trf("The voice isolator model was not found. Put its file (denoiser_realtime.safetensors) in {path}, or set KALANGO_AUDIOCPP_MODEL to it.", &[("path", shown(p))]));
        }
        lines.push(tr("Nothing is downloaded automatically: the model has its own licence, which you accept when you get it.").to_string());
        lines.join("\n")
    }
}

/// A path with the home folder written as `~`.
fn shown(p: &Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|h| p.strip_prefix(h).ok().map(|r| r.to_path_buf())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// Looks for the server program and the model (see `kalango_audiocpp::discovery` for where).
pub fn missing() -> Option<Missing> {
    let d = discovery::discover();
    if d.ready().is_some() {
        return None;
    }
    Some(Missing { server: d.server.err(), model: d.model.err() })
}

static SERVICE: Mutex<Option<Arc<Service>>> = Mutex::new(None);

/// The program-wide service, created (but not started: the server starts at the first use) when everything is
/// installed; otherwise what is missing.
pub fn service() -> Result<Arc<Service>, Missing> {
    let mut guard = SERVICE.lock().unwrap();
    if let Some(s) = guard.as_ref() {
        return Ok(s.clone());
    }
    let d = discovery::discover();
    let (server, model) = match (d.server, d.model) {
        (Ok(s), Ok(m)) => (s, m),
        (s, m) => return Err(Missing { server: s.err(), model: m.err() }),
    };
    let var = |k: &str| std::env::var(k).ok();
    // a crashed run leaves its small work folder behind (the child itself is killed with the app)
    Config::remove_stale_work_dirs();
    let mut cfg = Config::new(server, model, Config::private_work_dir());
    cfg.backend = discovery::choose_backend(&var, discovery::nvidia_present());
    cfg.threads = if cfg.backend.is_gpu() { 1 } else { std::thread::available_parallelism().map(|n| n.get().min(8) as u32).unwrap_or(4) };
    let service = Service::new(ServiceConfig::new(cfg));
    *guard = Some(service.clone());
    Ok(service)
}

/// Stops the server (if one runs) and forgets the service. Called when the app ends; the child would also die with the
/// app (the kernel kills it), this just does it politely and removes its private work folder.
pub fn shutdown() {
    let service = SERVICE.lock().unwrap().take();
    if let Some(s) = service {
        s.shutdown(Duration::from_secs(3));
    }
}

/// Asks a service that gave up to try again.
pub fn retry() {
    if let Some(s) = SERVICE.lock().unwrap().as_ref() {
        s.retry();
    }
}

/// The state of the live block, as the short code the engine passes to the window.
pub fn state_code(s: &LiveState) -> String {
    match s {
        LiveState::Starting => "starting".into(),
        LiveState::Running => "running".into(),
        LiveState::Fallback(why) => format!("fallback: {why}"),
        LiveState::Unavailable(why) => format!("error: {why}"),
        LiveState::OfflineOnly(LiveLimit::CpuOnly { .. }) => "offline: cpu".into(),
        LiveState::OfflineOnly(LiveLimit::TooSlow { rtf }) => format!("offline: slow {rtf:.1}"),
        LiveState::OfflineOnly(LiveLimit::NotRunning) => "starting".into(),
    }
}

/// The words for a [`state_code`] and whether it is a problem (shown in the error colour).
pub fn state_text(code: &str) -> (String, bool) {
    if code == "starting" {
        return (tr("Starting the voice model…").to_string(), false);
    }
    if code == "running" {
        return (tr("Isolating the voice").to_string(), false);
    }
    if let Some(why) = code.strip_prefix("fallback: ") {
        return (trf("Not isolating for now ({why}); the original sound is passing through.", &[("why", why.to_string())]), true);
    }
    if let Some(why) = code.strip_prefix("error: ") {
        return (trf("Voice isolation is not available ({why}); the original sound is passing through.", &[("why", why.to_string())]), true);
    }
    if code == "offline: cpu" {
        return (tr("This computer would run the model on the processor, which is too slow for live sound; the original sound is passing through. Use “Isolate voice” on a recording instead.").to_string(), true);
    }
    if let Some(x) = code.strip_prefix("offline: slow ") {
        return (trf("The model runs {x} times slower than the sound here, too slow for live use; the original sound is passing through. Use “Isolate voice” on a recording instead.", &[("x", x.replace('.', if crate::i18n::system_locale().is_some() { "," } else { "." }))]), true);
    }
    (code.to_string(), false)
}

/// "Graphics card (CUDA)", "Processor (CPU)".
pub fn backend_name(b: Backend) -> &'static str {
    match b {
        Backend::Cuda => tr("Graphics card (CUDA)"),
        Backend::Vulkan => tr("Graphics card (Vulkan)"),
        Backend::Cpu => tr("Processor (CPU)"),
    }
}

/// One line about the running server: where it runs and how fast ("Graphics card (CUDA) · 0.2 times the sound's
/// length"), or `None` while none runs.
pub fn server_line() -> Option<String> {
    let st = SERVICE.lock().unwrap().as_ref()?.status();
    match st.state {
        State::Running { backend, rtf } => Some(match rtf {
            Some(r) => trf("{backend} · takes {x} s per second of sound", &[("backend", backend_name(backend).to_string()), ("x", format!("{r:.2}").replace('.', if crate::i18n::system_locale().is_some() { "," } else { "." }))]),
            None => backend_name(backend).to_string(),
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------- offline: a recording

/// Where the isolated copy of a recording goes: beside it, "name (isolated).wav", numbered when it exists.
pub fn output_path(recording: &Path) -> PathBuf {
    let stem = recording.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "recording".into());
    let dir = recording.parent().unwrap_or(Path::new("."));
    let base = trf("{name} (isolated)", &[("name", stem)]);
    let first = dir.join(format!("{base}.wav"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{base} {n}.wav"))).find(|p| !p.exists()).unwrap()
}

/// The recording as mono 48 kHz samples (ffmpeg reads any format and resamples properly; without it only WAV files can
/// be read, and a WAV at another rate is resampled roughly).
pub fn decode(path: &Path) -> Result<Vec<f32>, String> {
    if crate::engine::ffmpeg_available() {
        let o = std::process::Command::new("ffmpeg")
            .args(["-v", "quiet", "-i"])
            .arg(path)
            .args(["-vn", "-ac", "1", "-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "-"])
            .output()
            .map_err(|e| e.to_string())?;
        if o.status.success() && !o.stdout.is_empty() {
            return Ok(o.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect());
        }
        return Err(tr("The recording could not be read.").to_string());
    }
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
        return crate::voz::read_mono_rate(path, SAMPLE_RATE).map_err(|_| tr("The recording could not be read.").to_string());
    }
    Err(tr("ffmpeg is needed to read this format").to_string())
}

/// Writes mono 48 kHz samples as a 24-bit WAV.
pub fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: SAMPLE_RATE, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(std::io::Error::other)?;
    for x in samples {
        w.write_sample((x.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32).map_err(std::io::Error::other)?;
    }
    w.finalize().map_err(std::io::Error::other)
}

pub struct Outcome {
    pub path: PathBuf,
    /// Length of the sound, in seconds.
    pub seconds: f64,
    /// Time the isolation took, in seconds.
    pub took: f64,
    pub backend: Backend,
}

/// Isolates the voice of a recording and saves the result beside it. `amount` 1 keeps only the isolated voice;
/// less lets some of the original back in. `progress` gets 0..1 (the first part is the model starting). Blocks: run it
/// on a thread.
pub fn isolate_recording(path: &Path, amount: f32, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<Outcome, String> {
    let service = service().map_err(|m| m.message())?;
    let _lease = service.lease();
    let st = service.wait_running(Duration::from_secs(180), cancel).map_err(|e| {
        if cancel.load(Ordering::Relaxed) {
            tr("Cancelled.").to_string()
        } else {
            trf("The voice model could not start: {why}", &[("why", e.to_string())])
        }
    })?;
    let backend = match st.state {
        State::Running { backend, .. } => backend,
        _ => Backend::Cpu,
    };
    let input = decode(path)?;
    if input.is_empty() {
        return Err(tr("The recording has no sound.").to_string());
    }
    let seconds = input.len() as f64 / SAMPLE_RATE as f64;
    let result = offline::isolate(&service, &input, |f| {
        progress(f);
        !cancel.load(Ordering::Relaxed)
    })
    .map_err(|e| if cancel.load(Ordering::Relaxed) { tr("Cancelled.").to_string() } else { trf("Voice isolation failed: {why}", &[("why", e.to_string())]) })?;
    let samples = if amount >= 1.0 { result.samples } else { offline::blend(&input, &result.samples, amount) };
    let out = output_path(path);
    write_wav(&out, &samples).map_err(|e| trf("The result could not be saved: {why}", &[("why", e.to_string())]))?;
    Ok(Outcome { path: out, seconds, took: result.elapsed.as_secs_f64(), backend })
}

/// Tests that change the environment (the server and model paths) or use the program-wide service take this lock.
#[cfg(test)]
pub static TEST_ENV: Mutex<()> = Mutex::new(());

/// The fake audiocpp server the wrapper's own tests use, when it has been built (`cargo test --workspace`).
#[cfg(test)]
pub fn fake_server() -> Option<PathBuf> {
    let debug_dir = std::env::current_exe().ok()?.parent()?.parent()?.to_path_buf();
    Some(debug_dir.join("fake_audiocpp_server")).filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_what_is_missing_and_where_it_goes() {
        let m = Missing { server: Some(PathBuf::from("/x/bin/audiocpp_server")), model: None };
        let text = m.message();
        assert!(text.contains("/x/bin/audiocpp_server") && text.contains("KALANGO_AUDIOCPP_SERVER"));
        assert!(!text.contains("denoiser_realtime"), "the model is not missing");
        assert!(text.contains("Nothing is downloaded automatically"));
        let both = Missing { server: Some("/a".into()), model: Some("/b".into()) }.message();
        assert!(both.contains("/a") && both.contains("/b") && both.contains("KALANGO_AUDIOCPP_MODEL"));
    }

    #[test]
    fn state_codes_round_trip_into_words() {
        assert_eq!(state_text(&state_code(&LiveState::Running)), ("Isolating the voice".to_string(), false));
        assert!(!state_text("starting").1);
        let (t, bad) = state_text(&state_code(&LiveState::Fallback("the server stopped".into())));
        assert!(bad && t.contains("the server stopped") && t.contains("passing through"));
        let (t, bad) = state_text(&state_code(&LiveState::Unavailable("no GPU memory".into())));
        assert!(bad && t.contains("no GPU memory"));
        let (t, bad) = state_text(&state_code(&LiveState::OfflineOnly(LiveLimit::CpuOnly { rtf: Some(3.2) })));
        assert!(bad && t.contains("processor") && t.contains("Isolate voice"));
        let (t, _) = state_text(&state_code(&LiveState::OfflineOnly(LiveLimit::TooSlow { rtf: 1.46 })));
        assert!(t.contains("1.5") || t.contains("1,5"), "{t}");
    }

    #[test]
    fn the_isolated_copy_goes_beside_the_recording_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("kalango-iso-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = dir.join("take.flac");
        assert_eq!(output_path(&rec), dir.join("take (isolated).wav"));
        std::fs::write(dir.join("take (isolated).wav"), b"x").unwrap();
        assert_eq!(output_path(&rec), dir.join("take (isolated) 2.wav"));
    }

    fn noise_wav(path: &Path, seconds: usize) -> Vec<f32> {
        let mut s = 11u32;
        let x: Vec<f32> = (0..SAMPLE_RATE as usize * seconds)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                0.6 * ((s >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
            })
            .collect();
        write_wav(path, &x).unwrap();
        x
    }

    /// The recording action: missing pieces are explained, nothing is written; with a server the isolated copy is saved
    /// beside the recording (and the mix amount is honoured); a cancel stops it.
    #[test]
    fn isolating_a_recording_explains_what_is_missing_then_saves_a_copy_beside_it() {
        let _lock = TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("kalango-iso-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = dir.join("take.wav");
        let input = noise_wav(&rec, 3);
        let cancel = AtomicBool::new(false);

        std::env::set_var("KALANGO_AUDIOCPP_SERVER", "/nonexistent/audiocpp_server");
        std::env::set_var("KALANGO_AUDIOCPP_MODEL", "/nonexistent/model");
        shutdown();
        let Err(e) = isolate_recording(&rec, 1.0, &cancel, |_| {}) else { panic!("nothing is installed") };
        assert!(e.contains("KALANGO_AUDIOCPP_SERVER") && e.contains("KALANGO_AUDIOCPP_MODEL"), "{e}");
        assert!(!dir.join("take (isolated).wav").exists(), "nothing is written when it cannot run");

        let Some(fake) = fake_server() else {
            eprintln!("skipped the server half: build the fake first (cargo test --workspace)");
            return;
        };
        std::env::set_var("KALANGO_AUDIOCPP_SERVER", &fake);
        std::env::set_var("KALANGO_AUDIOCPP_MODEL", std::env::current_exe().unwrap());
        std::env::set_var("KALANGO_AUDIOCPP_BACKEND", "cuda");
        let mut fractions = Vec::new();
        let done = isolate_recording(&rec, 1.0, &cancel, |f| fractions.push(f)).expect("isolated");
        assert_eq!(done.path, dir.join("take (isolated).wav"));
        assert!((done.seconds - 3.0).abs() < 0.01);
        assert!(fractions.windows(2).all(|w| w[0] <= w[1]) && fractions.last().is_some_and(|f| *f > 0.99));
        let out = decode(&done.path).unwrap();
        assert_eq!(out.len(), input.len(), "as long as the recording");
        // the fake isolator halves every sample; the 24-bit file keeps that
        let worst = out.iter().zip(&input).map(|(o, i)| (o - 0.5 * i).abs()).fold(0.0f32, f32::max);
        assert!(worst < 5e-4, "{worst}");
        // half the original back in: 0.5 * 0.5x + 0.5 * x
        let mixed = isolate_recording(&rec, 0.5, &cancel, |_| {}).expect("mixed");
        assert_eq!(mixed.path, dir.join("take (isolated) 2.wav"), "a second copy never overwrites the first");
        let out = decode(&mixed.path).unwrap();
        assert!((out[5000] - 0.75 * input[5000]).abs() < 5e-4);
        // cancelled
        cancel.store(true, Ordering::Relaxed);
        let Err(e) = isolate_recording(&rec, 1.0, &cancel, |_| {}) else { panic!("cancelled") };
        assert!(e.contains("Cancelled"), "{e}");
        shutdown();
    }

    #[test]
    fn a_wav_round_trips_through_write_and_decode() {
        let dir = std::env::temp_dir().join(format!("kalango-iso-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        let x: Vec<f32> = (0..4800).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
        write_wav(&p, &x).unwrap();
        let back = decode(&p).unwrap();
        assert_eq!(back.len(), x.len());
        assert!(back.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-5));
    }
}
