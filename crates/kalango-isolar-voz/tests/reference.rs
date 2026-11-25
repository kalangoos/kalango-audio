//! The Rust port against the PyTorch reference, with the real released weights.
//!
//! Needs `KALANGO_ISOLAR_VOZ_DIR` (default `~/.cache/kalango-isolar-voz`) holding
//! `weights/denoiser*.safetensors` (from Hugging Face `inworld/voice-isolator`) and `ref/` (inputs and PyTorch outputs
//! written by this crate's `scripts/make_reference.py` with the upstream PyTorch package on CPU:
//! `python -I scripts/make_reference.py <voice-isolator clone> <dir>/ref <dir>/weights`). Without them each test
//! says so and passes.

use std::path::{Path, PathBuf};

use kalango_isolar_voz::metrics::{max_abs_diff, read_f32, si_sdr, snr};
use kalango_isolar_voz::{Offline, Realtime, RealtimeOptions, OFFLINE_FILE, REALTIME_FILE};

const CLIPS: [&str; 4] = ["pink_0db_3s", "music_5db_3s", "babble_10db_3s", "pink_5db_12s"];

fn base() -> PathBuf {
    std::env::var_os("KALANGO_ISOLAR_VOZ_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/kalango-isolar-voz")
    })
}

fn available(file: &str) -> Option<PathBuf> {
    let weights = base().join("weights").join(file);
    let reference = base().join("ref").join(format!("{}.in.f32", CLIPS[0]));
    if weights.exists() && reference.exists() {
        Some(weights)
    } else {
        eprintln!(
            "pulado: pesos reais ou referência ausentes em {} (baixe inworld/voice-isolator e rode make_reference.py)",
            base().display()
        );
        None
    }
}

fn clip(name: &str, kind: &str) -> Vec<f32> {
    read_f32(&base().join("ref").join(format!("{name}.{kind}.f32"))).unwrap()
}

fn threads() -> usize {
    std::env::var("ISOLAR_VOZ_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(4)
}

fn check(name: &str, ours: &[f32], torch: &[f32], clean: &[f32]) {
    assert_eq!(ours.len(), torch.len(), "{name}: comprimento");
    let err = max_abs_diff(ours, torch);
    let parity = si_sdr(ours, torch);
    let (a, b) = (si_sdr(ours, clean), si_sdr(torch, clean));
    eprintln!("{name}: erro máx {err:.2e}, SI-SDR(rust, torch) {parity:.1} dB, SI-SDR vs limpo rust {a:.3} / torch {b:.3} dB (Δ {:.4})", a - b);
    assert!((a - b).abs() < 0.1, "{name}: SI-SDR difere {:.4} dB", a - b);
    assert!(parity > 40.0, "{name}: paridade {parity:.1} dB");
}

#[test]
fn offline_matches_pytorch() {
    let Some(weights) = available(OFFLINE_FILE) else { return };
    let mut model = Offline::open(&weights, threads()).unwrap();
    for name in CLIPS {
        let input = clip(name, "in");
        let ours = model.process(&input);
        check(&format!("offline/{name}"), &ours, &clip(name, "offline"), &clip(name, "clean"));
    }
}

fn stream(weights: &Path, input: &[f32]) -> Vec<f32> {
    let mut rt = Realtime::open(weights, RealtimeOptions { threads: threads(), ..Default::default() }).unwrap();
    let mut out = Vec::with_capacity(input.len());
    for piece in input.chunks(960) {
        rt.push(piece, &mut out);
    }
    rt.flush(&mut out);
    out
}

#[test]
fn realtime_matches_pytorch() {
    let Some(weights) = available(REALTIME_FILE) else { return };
    for name in CLIPS {
        let input = clip(name, "in");
        let ours = stream(&weights, &input);
        check(&format!("realtime/{name}"), &ours, &clip(name, "realtime"), &clip(name, "clean"));
    }
}

#[test]
fn push_frame_is_the_stream_delayed() {
    let Some(weights) = available(REALTIME_FILE) else { return };
    let input = clip(CLIPS[0], "in");
    let streamed = stream(&weights, &input);
    let mut rt = Realtime::open(&weights, RealtimeOptions { threads: threads(), ..Default::default() }).unwrap();
    let delay = rt.frame_latency(960);
    assert_eq!(delay, 2944);
    let mut out = vec![0f32; 960];
    let mut got = Vec::new();
    for piece in input.as_chunks::<960>().0 {
        rt.push_frame(piece, &mut out);
        got.extend_from_slice(&out);
    }
    assert!(got[..delay].iter().all(|v| *v == 0.0));
    let n = got.len() - delay;
    assert_eq!(max_abs_diff(&got[delay..], &streamed[..n]), 0.0);
}

fn tensor(kind: &str, stage: &str) -> Vec<f32> {
    read_f32(&base().join("inter").join(format!("{kind}.{stage}.f32"))).unwrap()
}

/// `[frame][bin][k]` to the reference's `[bin][frame][k]`.
fn frames_to_bins(x: &[f32], frames: usize, k: usize) -> Vec<f32> {
    let bins = x.len() / (frames * k);
    let mut out = vec![0f32; x.len()];
    for t in 0..frames {
        for b in 0..bins {
            out[(b * frames + t) * k..(b * frames + t + 1) * k]
                .copy_from_slice(&x[(t * bins + b) * k..(t * bins + b + 1) * k]);
        }
    }
    out
}

/// Stage by stage on the first clip: where the error appears and how it grows. Thresholds: f32 arithmetic with another
/// summation order (AVX2 FMA tiles here, MKL/oneDNN in PyTorch) leaves about 1e-6 relative error per layer, so a layer
/// that is right stays above 100 dB; 60 dB (relative RMS error 1e-3) over 7 blocks of attention/Mamba still catches any
/// wrong layer (a mistake in one head, a scan direction or a norm epsilon costs tens of dB) but not rounding noise.
fn stages(file: &str, kind: &str, has_stft: bool, last: &str) {
    let Some(weights) = available(file) else { return };
    let mut model = Offline::open(&weights, threads()).unwrap();
    let trace = model.trace(&clip(CLIPS[0], "in"));
    let mut worst = f64::MAX;
    let mut report = |name: &str, ours: &[f32], torch: &[f32]| {
        assert_eq!(ours.len(), torch.len(), "{kind}/{name}: size");
        let (err, db) = (max_abs_diff(ours, torch), snr(ours, torch));
        eprintln!("{kind}/{name}: erro máx {err:.2e}, SNR {db:.1} dB");
        worst = worst.min(db);
    };
    if has_stft {
        let spec: Vec<f32> = trace.spec.iter().flat_map(|c| [c.re, c.im]).collect();
        report("stft", &frames_to_bins(&spec, trace.frames, 2), &tensor(kind, "stft"));
    }
    report("encode", &trace.stages[0], &tensor(kind, "encode"));
    for (i, stage) in trace.stages[1..].iter().enumerate() {
        report(&format!("block{i}"), stage, &tensor(kind, &format!("block{i}")));
    }
    report("raw", &frames_to_bins(&trace.raw, trace.frames, 4), &tensor(kind, "raw"));
    report("saída", &trace.out, &tensor(kind, last));
    assert!(worst > 60.0, "{kind}: pior estágio {worst:.1} dB");
}

#[test]
fn offline_stages_match_pytorch() {
    stages(OFFLINE_FILE, "offline", true, "out");
}

#[test]
fn realtime_stages_match_pytorch() {
    stages(REALTIME_FILE, "realtime", false, "forward");
}
