//! The service layer against the real `audiocpp_server` and the real weights. Ignored by default; run with
//!
//! ```text
//! KALANGO_AUDIOCPP_SERVER=/abs/audiocpp_server KALANGO_AUDIOCPP_MODEL=/abs/denoiser_realtime.safetensors \
//!   [KALANGO_AUDIOCPP_BACKEND=cuda|vulkan|cpu] [KALANGO_AUDIOCPP_REF_DIR=~/.cache/kalango-isolar-voz/ref] \
//!   cargo test -p kalango-audiocpp --test real_service -- --ignored --nocapture
//! ```
//! Without the variables (or without the reference clip) the test prints why it skips and passes.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use kalango_audiocpp::live::{LiveIsolator, LiveState};
use kalango_audiocpp::offline;
use kalango_audiocpp::service::{Service, ServiceConfig, State};
use kalango_audiocpp::{Backend, Config, SAMPLE_RATE};
use kalango_isolar_voz::metrics::{read_f32, si_sdr};

fn config() -> Option<ServiceConfig> {
    let server = std::env::var_os("KALANGO_AUDIOCPP_SERVER").map(PathBuf::from);
    let model = std::env::var_os("KALANGO_AUDIOCPP_MODEL").map(PathBuf::from);
    let (Some(server), Some(model)) = (server, model) else {
        eprintln!("skipped: set KALANGO_AUDIOCPP_SERVER and KALANGO_AUDIOCPP_MODEL");
        return None;
    };
    let mut cfg =
        Config::new(server, model, std::env::temp_dir().join(format!("kalango-real-service-{}", std::process::id())));
    cfg.backend = match std::env::var("KALANGO_AUDIOCPP_BACKEND").as_deref() {
        Ok("vulkan") => Backend::Vulkan,
        Ok("cpu") => Backend::Cpu,
        _ => Backend::Cuda,
    };
    cfg.threads = if cfg.backend.is_gpu() { 1 } else { 8 };
    cfg.startup_timeout = Duration::from_secs(120);
    Some(ServiceConfig::new(cfg))
}

/// `<name>.in.f32` (noisy input) and `<name>.realtime.f32` (what the reference model made of it).
fn reference_clip(name: &str) -> Option<(Vec<f32>, Vec<f32>)> {
    let dir = std::env::var_os("KALANGO_AUDIOCPP_REF_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/kalango-isolar-voz/ref")))?;
    let input = read_f32(&dir.join(format!("{name}.in.f32"))).ok()?;
    let reference = read_f32(&dir.join(format!("{name}.realtime.f32"))).ok()?;
    Some((input, reference))
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok()
}

fn rss_mib(pid: u32) -> f64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0)
        / 1024.0
}

#[test]
#[ignore]
fn the_service_isolates_the_reference_clip_offline_and_live() {
    let Some(cfg) = config() else { return };
    let Some((input, reference)) = reference_clip("pink_5db_12s") else {
        eprintln!("skipped: no reference clip (set KALANGO_AUDIOCPP_REF_DIR to the folder with pink_5db_12s.in.f32)");
        return;
    };
    let service = Service::new(cfg);
    assert_eq!(service.status().state, State::Idle, "nothing runs before the first use");
    let t_start = Instant::now();
    let lease = service.lease();
    let st = service.wait_running(Duration::from_secs(120), &AtomicBool::new(false)).expect("running");
    let State::Running { backend, rtf } = st.state.clone() else { unreachable!() };
    let pid = st.pid.unwrap();
    eprintln!(
        "server pid {pid}, backend {backend:?}, probe rtf {rtf:?}, rss {:.0} MiB, ready {:.2} s after the first use",
        rss_mib(pid),
        t_start.elapsed().as_secs_f32()
    );

    // offline: as fast as the server takes the sound
    let isolated = offline::isolate(&service, &input, |_| true).expect("offline isolation");
    let sdr = si_sdr(&isolated.samples, &reference);
    eprintln!(
        "offline: {} s of sound in {:.2} s (rtf {:.3}), SI-SDR vs the stored reference {sdr:.1} dB",
        input.len() / SAMPLE_RATE as usize,
        isolated.elapsed.as_secs_f32(),
        isolated.rtf()
    );
    assert_eq!(isolated.samples.len(), input.len());
    let body = input.len() - 8192;
    eprintln!(
        "offline: SI-SDR of the first {} samples {:.1} dB, of the last 8192 {:.1} dB",
        body,
        si_sdr(&isolated.samples[..body], &reference[..body]),
        si_sdr(&isolated.samples[body..], &reference[body..])
    );
    assert!(
        si_sdr(&isolated.samples[..body], &reference[..body]) > 60.0,
        "the body of the clip must match the stored reference"
    );
    assert!(sdr > 40.0, "the isolated clip must match the stored reference: {sdr} dB");

    // live: paced like the audio engine, 10 ms at a time, only when the server runs on a graphics card. The graphics
    // card is shared with other programs, and a late answer makes the delay grow by the gap (by design), so the
    // check looks at the last 4 s of a run and may take up to three runs to get one without a late answer in them.
    if backend.is_gpu() {
        let mut passed = false;
        for attempt in 1..=3 {
            let mut live = LiveIsolator::new(service.clone());
            let until = Instant::now() + Duration::from_secs(30);
            while live.state() != LiveState::Running {
                assert!(Instant::now() < until, "live never started: {:?}", live.state());
                std::thread::sleep(Duration::from_millis(20));
            }
            let mut out = Vec::new();
            let mut latencies = Vec::new();
            let mut next = Instant::now();
            for chunk in input.chunks(480) {
                let mut frames: Vec<[f32; 2]> = chunk.iter().map(|s| [*s, *s]).collect();
                live.process(&mut frames, 1.0);
                out.extend(frames.iter().map(|f| f[0]));
                latencies.push(live.latency_ms());
                next += Duration::from_millis(10);
                if let Some(d) = next.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
            }
            let last = *latencies.last().unwrap();
            latencies.sort_by(|a, b| a.total_cmp(b));
            let (p50, p99) = (latencies[latencies.len() / 2], latencies[latencies.len() * 99 / 100]);
            // the output is the isolated sound delayed by the latency: find the delay that fits the reference best
            let steady = (last * SAMPLE_RATE as f32 / 1000.0) as usize;
            let (mut best_delay, mut best_sdr) = (0usize, f64::MIN);
            let (from, to) = (input.len() - SAMPLE_RATE as usize * 4 - 8192, input.len() - 8192);
            for delay in steady.saturating_sub(1500)..steady + 1500 {
                if delay + to > out.len() {
                    continue;
                }
                let s = si_sdr(&out[delay + from..delay + to], &reference[from..to]);
                if s > best_sdr {
                    (best_delay, best_sdr) = (delay, s);
                }
            }
            eprintln!(
                "live run {attempt}: delay p50 {p50:.1} ms, p99 {p99:.1} ms, last {last:.1} ms; the last 4 s fit the reference at {best_delay} samples ({:.1} ms) with SI-SDR {best_sdr:.1} dB; late answers {}",
                best_delay as f32 * 1000.0 / SAMPLE_RATE as f32,
                live.gaps()
            );
            drop(live);
            if best_sdr > 30.0 && p50 < 250.0 {
                passed = true;
                break;
            }
        }
        assert!(passed, "no live run matched the reference at a fixed delay");
    } else {
        eprintln!("live skipped: the server runs on the CPU");
    }
    drop(lease);
    service.shutdown(Duration::from_secs(10));
    assert!(!alive(pid), "the server is gone after shutdown");
}
