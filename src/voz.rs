//! Speech to text with KalangoOS's own local voice model (LFM3-Audio-2B), through the system's agent program:
//! `kalango-agente --json transcrever <file.wav>` answers one JSON line, `{"tipo": "transcricao", "texto": …}` or
//! `{"tipo": "erro", "texto": …}`. The agent starts the model when needed and lets it go when idle; the language is
//! the system's (the agent reads the locale). Nothing leaves the computer.
//!
//! Long audio goes in slices, so a live session's transcript grows while it records and a long recording shows
//! its first lines early; each line carries the time of its slice: "[mm:ss] text".

use crate::i18n::tr;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The voice model's file (as the agent names it): an error that mentions it means the model is missing.
const AUDIO_MODEL_HINT: &str = "LFM3-Audio";

/// Seconds of sound per request: short enough for the text to appear live, long enough for whole sentences.
pub const SLICE_SECONDS: usize = 15;
/// The sample rate the slices are written at (the model works on speech; 16 kHz mono keeps requests small).
pub const RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceError {
    /// the agent program, or the voice model it uses, is not installed
    NotInstalled,
    /// anything else, in the agent's words
    Other(String),
}

impl VoiceError {
    /// What the person reads.
    pub fn message(&self) -> String {
        match self {
            VoiceError::NotInstalled => tr("The voice model is not installed — get it in the App Center").to_string(),
            VoiceError::Other(e) => e.clone(),
        }
    }
}

/// Where the agent program is: `$KALANGO_AGENTE`, the system's /usr/libexec/kalango-agente, or `kalango-agente` on
/// the PATH (the development session puts its build there).
pub fn agent_binary() -> Option<PathBuf> {
    agent_binary_in(std::env::var_os("KALANGO_AGENTE").map(PathBuf::from), std::env::var_os("PATH"))
}

fn agent_binary_in(env: Option<PathBuf>, path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(p) = env.filter(|p| p.is_file()) {
        return Some(p);
    }
    let system = PathBuf::from("/usr/libexec/kalango-agente");
    if system.is_file() {
        return Some(system);
    }
    path.and_then(|p| std::env::split_paths(&p).map(|d| d.join("kalango-agente")).find(|f| f.is_file()))
}

/// The voice model's files (the model and its audio projector), as the agent looks for them.
const MODEL_FILES: [&str; 2] = ["LFM3-Audio-2B-Q4_K_M.gguf", "mmproj-LFM3-Audio-2B-Q8_0.gguf"];

/// Whether the voice model is installed (the same folders as the agent: $KALANGO_MODELOS, the person's data folder,
/// the system's).
pub fn model_installed() -> bool {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    let dirs: Vec<PathBuf> = std::env::var_os("KALANGO_MODELOS")
        .map(PathBuf::from)
        .into_iter()
        .chain([data.join("kalango/modelos"), PathBuf::from("/usr/share/kalango/modelos")])
        .collect();
    MODEL_FILES.iter().all(|f| dirs.iter().any(|d| d.join(f).is_file()))
}

/// What can be said about transcription here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    Ready,
    NoAgent,
    NoModel,
}

/// (looked up again at most every 5 s: the editor asks on every redraw)
pub fn status() -> Status {
    use std::sync::Mutex;
    static CACHE: Mutex<Option<(std::time::Instant, Status)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((t, s)) = *c {
        if t.elapsed().as_secs() < 5 {
            return s;
        }
    }
    let s = status_now();
    *c = Some((std::time::Instant::now(), s));
    s
}

fn status_now() -> Status {
    if agent_binary().is_none() {
        Status::NoAgent
    } else if !model_installed() {
        Status::NoModel
    } else {
        Status::Ready
    }
}

/// Reads the agent's answer (its last JSON line on stdout, with stderr for when it printed nothing).
pub fn parse_answer(stdout: &str, stderr: &str) -> Result<String, VoiceError> {
    let line = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
    let v: serde_json::Value = serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
    match v["tipo"].as_str() {
        Some("transcricao") => Ok(v["texto"].as_str().unwrap_or("").trim().to_string()),
        _ => {
            let why = v["texto"].as_str().map(str::to_string).unwrap_or_else(|| stderr.trim().lines().last().unwrap_or("").to_string());
            if why.contains(AUDIO_MODEL_HINT) {
                Err(VoiceError::NotInstalled)
            } else if why.is_empty() {
                Err(VoiceError::Other(tr("the voice model did not answer").to_string()))
            } else {
                Err(VoiceError::Other(why))
            }
        }
    }
}

/// One WAV file to text (blocking: call it off the window's thread).
pub fn transcribe_wav(path: &Path) -> Result<String, VoiceError> {
    let bin = agent_binary().ok_or(VoiceError::NotInstalled)?;
    let o = std::process::Command::new(bin)
        .args(["--json", "transcrever"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| VoiceError::Other(e.to_string()))?;
    parse_answer(&String::from_utf8_lossy(&o.stdout), &String::from_utf8_lossy(&o.stderr))
}

/// "[mm:ss] text" (or "[h:mm:ss]" past an hour): the transcript format the Recordings screen reads.
pub fn line(start_seconds: f64, text: &str) -> String {
    let s = start_seconds.max(0.0) as u64;
    let stamp = if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) };
    format!("[{stamp}] {}", text.trim())
}

/// Writes mono samples (−1..1, at [`RATE`]) to a 16-bit WAV.
pub fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(std::io::Error::other)?;
    for x in samples {
        w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(std::io::Error::other)?;
    }
    w.finalize().map_err(std::io::Error::other)
}

/// Whether a slice holds anything worth sending (pure silence is skipped: the model would only invent words).
pub fn has_sound(samples: &[f32]) -> bool {
    samples.iter().any(|x| x.abs() > 0.003)
}

/// A recording as 16 kHz mono samples: WAV directly, other formats through ffmpeg.
pub fn read_mono(path: &Path) -> Result<Vec<f32>, String> {
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
    if ext == "wav" {
        let mut r = hound::WavReader::open(path).map_err(|e| e.to_string())?;
        let spec = r.spec();
        let ch = spec.channels.max(1) as usize;
        let all: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().filter_map(Result::ok).collect(),
            hound::SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample.max(1) - 1)) as f32;
                r.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / max).collect()
            }
        };
        let mono: Vec<f32> = all.chunks(ch).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
        return Ok(resample(&mono, spec.sample_rate, RATE));
    }
    let o = std::process::Command::new("ffmpeg")
        .args(["-v", "quiet", "-i"])
        .arg(path)
        .args(["-ac", "1", "-ar", &RATE.to_string(), "-f", "s16le", "-"])
        .output()
        .map_err(|_| tr("ffmpeg is needed to read this format").to_string())?;
    Ok(o.stdout.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect())
}

/// Linear resampling (enough for speech recognition).
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let n = (x.len() as u64 * to as u64 / from as u64) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 * from as f64 / to as f64;
            let k = t.floor() as usize;
            let f = (t - k as f64) as f32;
            let a = x[k.min(x.len() - 1)];
            let b = x[(k + 1).min(x.len() - 1)];
            a + (b - a) * f
        })
        .collect()
}

/// Transcribes samples in slices, appending "[mm:ss] text" lines to `out` as each slice is done; `offset` is the
/// time of the first sample. Stops at the first "not installed" (no use asking again).
pub fn transcribe_slices(samples: &[f32], offset: f64, out: &Path, tmp: &Path, mut on_line: impl FnMut(&str)) -> Result<usize, VoiceError> {
    let per = SLICE_SECONDS * RATE as usize;
    let mut lines = 0;
    for (i, slice) in samples.chunks(per).enumerate() {
        if !has_sound(slice) {
            continue;
        }
        write_wav(tmp, slice).map_err(|e| VoiceError::Other(e.to_string()))?;
        let text = transcribe_wav(tmp);
        let _ = std::fs::remove_file(tmp);
        let text = text?;
        if text.is_empty() {
            continue;
        }
        let l = line(offset + (i * SLICE_SECONDS) as f64, &text);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(out) {
            let _ = writeln!(f, "{l}");
        }
        on_line(&l);
        lines += 1;
    }
    Ok(lines)
}

/// A finished recording to text, next to it (`<recording>.txt`), in the background. Returns the text file it will
/// write, or why it can't start.
pub fn transcribe_recording(path: &str) -> Result<PathBuf, String> {
    let src = PathBuf::from(path);
    if !src.is_file() {
        return Err(crate::i18n::trf("the recording “{path}” does not exist", &[("path", path.to_string())]));
    }
    if agent_binary().is_none() {
        return Err(VoiceError::NotInstalled.message());
    }
    let txt = src.with_extension("txt");
    let out = txt.clone();
    std::thread::spawn(move || {
        let Ok(samples) = read_mono(&src) else { return };
        let _ = std::fs::remove_file(&out);
        let tmp = std::env::temp_dir().join(format!("kalango-audio-voz-{}-{}.wav", std::process::id(), crate::model::new_uid()));
        let _ = transcribe_slices(&samples, 0.0, &out, &tmp, |_| {});
    });
    Ok(txt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_answer_is_read_and_a_missing_model_is_recognised() {
        assert_eq!(parse_answer("· iniciando…\n{\"texto\":\"Bom dia.\",\"tipo\":\"transcricao\"}\n", ""), Ok("Bom dia.".into()));
        let missing = "{\"tipo\":\"erro\",\"texto\":\"o modelo de voz não está instalado (LFM3-Audio-2B-Q4_K_M.gguf em ~/.local/share/kalango/modelos)\"}";
        assert_eq!(parse_answer(missing, ""), Err(VoiceError::NotInstalled));
        assert_eq!(parse_answer("{\"tipo\":\"erro\",\"texto\":\"llama-server not found\"}", ""), Err(VoiceError::Other("llama-server not found".into())));
        assert!(matches!(parse_answer("", "kalango-agente: something broke"), Err(VoiceError::Other(e)) if e.contains("something broke")));
        assert!(VoiceError::NotInstalled.message().contains("App Center"));
    }

    #[test]
    fn the_agent_is_found_by_env_system_or_path_only() {
        let d = crate::engine::test_home().join("agente");
        std::fs::create_dir_all(&d).unwrap();
        let bin = d.join("kalango-agente");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        assert_eq!(agent_binary_in(Some(bin.clone()), None), Some(bin.clone()));
        assert_eq!(agent_binary_in(None, Some(d.clone().into_os_string())), if Path::new("/usr/libexec/kalango-agente").is_file() { Some("/usr/libexec/kalango-agente".into()) } else { Some(bin.clone()) });
        let empty = crate::engine::test_home().join("vazio");
        std::fs::create_dir_all(&empty).unwrap();
        if !Path::new("/usr/libexec/kalango-agente").is_file() {
            assert_eq!(agent_binary_in(Some(empty.join("nope")), Some(empty.into_os_string())), None);
        }
    }

    #[test]
    fn lines_carry_the_slice_time() {
        assert_eq!(line(0.0, " Bom dia. "), "[00:00] Bom dia.");
        assert_eq!(line(75.0, "a"), "[01:15] a");
        assert_eq!(line(3725.0, "b"), "[1:02:05] b");
    }

    #[test]
    fn silence_is_not_sent_and_resampling_keeps_the_length() {
        assert!(!has_sound(&[0.0; 1000]));
        assert!(has_sound(&[0.0, 0.2, 0.0]));
        assert_eq!(resample(&vec![0.5; 48_000], 48_000, 16_000).len(), 16_000);
        assert_eq!(resample(&vec![0.5; 22_050], 22_050, 16_000).len(), 16_000);
    }

    /// The real model (as the KalangoOS rules ask: never a fake one): skipped where the agent or the model is not
    /// installed. Reads a generated speech file if one is given (KALANGO_AUDIO_FALA_TESTE), never a microphone.
    #[test]
    fn transcribes_speech_with_the_system_voice_model() {
        let Some(wav) = std::env::var_os("KALANGO_AUDIO_FALA_TESTE").map(PathBuf::from).filter(|p| p.is_file()) else { return };
        if agent_binary().is_none() {
            return;
        }
        let samples = read_mono(&wav).unwrap();
        let _ = std::fs::create_dir_all(crate::engine::test_home());
        let out = crate::engine::test_home().join("fala.txt");
        let _ = std::fs::remove_file(&out);
        let tmp = crate::engine::test_home().join("fatia.wav");
        let mut got = vec![];
        let n = transcribe_slices(&samples, 0.0, &out, &tmp, |l| got.push(l.to_string())).expect("transcription");
        assert!(n >= 1, "{got:?}");
        assert!(got[0].starts_with("[00:00] "), "{got:?}");
        assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), n);
    }
}
