//! The service layer against the fake server (see `tests/support/fake_server.rs`): lazy start, restarts, idle stop,
//! no orphans, offline isolation and the live mix with its alignment.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kalango_audiocpp::live::{LiveIsolator, LiveState};
use kalango_audiocpp::offline;
use kalango_audiocpp::service::{LiveLimit, Service, ServiceConfig, State};
use kalango_audiocpp::{Backend, Config, Error, SAMPLE_RATE};

fn config(tag: &str, env: &[(&str, &str)]) -> ServiceConfig {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_fake_audiocpp_server"));
    let dir = std::env::temp_dir().join(format!("kalango-service-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut server = Config::new(bin, "/unused".into(), dir);
    server.startup_timeout = Duration::from_secs(10);
    server.env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let mut cfg = ServiceConfig::new(server);
    cfg.probe = false;
    cfg.backoff_base = Duration::from_millis(100);
    cfg.backoff_max = Duration::from_millis(400);
    cfg
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok()
}

fn wait_for(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for: {what}");
}

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            0.6 * ((s >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
        })
        .collect()
}

fn running(service: &Arc<Service>) -> kalango_audiocpp::service::Status {
    service.wait_running(Duration::from_secs(10), &AtomicBool::new(false)).expect("running")
}

#[test]
fn nothing_starts_until_somebody_asks() {
    let service = Service::new(config("lazy", &[]));
    std::thread::sleep(Duration::from_millis(300));
    let st = service.status();
    assert_eq!((st.state, st.pid, st.starts), (State::Idle, None, 0));
    let lease = service.lease();
    let st = running(&service);
    assert!(st.pid.is_some() && st.starts == 1);
    drop(lease);
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn the_server_stops_after_the_idle_time_and_starts_again_on_demand() {
    let mut cfg = config("idle", &[]);
    cfg.idle_timeout = Duration::from_millis(400);
    let service = Service::new(cfg);
    let lease = service.lease();
    let pid = running(&service).pid.unwrap();
    let other = service.lease();
    drop(lease);
    std::thread::sleep(Duration::from_millis(900));
    assert!(alive(pid), "a second user keeps it running");
    drop(other);
    wait_for("the idle stop", 5, || service.status().state == State::Idle);
    assert!(!alive(pid), "the process is gone, reaped");
    let _again = service.lease();
    let st = running(&service);
    assert_eq!(st.starts, 2);
    assert_ne!(st.pid.unwrap(), pid);
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn shutdown_leaves_no_process_and_no_files() {
    let cfg = config("down", &[]);
    let dir = cfg.server.work_dir.clone();
    let service = Service::new(cfg);
    let _lease = service.lease();
    let pid = running(&service).pid.unwrap();
    assert!(dir.exists());
    service.shutdown(Duration::from_secs(5));
    assert!(!alive(pid));
    assert!(!dir.exists(), "the private work folder is removed");
    assert!(service.open_stream().is_err());
}

#[test]
fn dropping_the_service_reaps_the_child() {
    let service = Service::new(config("dropped", &[]));
    let lease = service.lease();
    let pid = running(&service).pid.unwrap();
    drop(lease);
    drop(service);
    assert!(!alive(pid));
}

#[test]
fn the_backend_comes_from_what_the_server_reports() {
    // asked for CUDA, the (fake) server says it runs on the CPU
    let service = Service::new(config("cpu", &[("FAKE_BACKEND", "cpu")]));
    let _lease = service.lease();
    let st = running(&service);
    assert!(matches!(st.state, State::Running { backend: Backend::Cpu, .. }));
    assert_eq!(st.live_ok(), Err(LiveLimit::CpuOnly { rtf: None }));
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn the_speed_probe_measures_the_server() {
    let mut cfg = config("probe", &[("FAKE_MS_PER_CHUNK", "40")]);
    cfg.probe = true;
    let service = Service::new(cfg);
    let _lease = service.lease();
    let st = running(&service);
    // 50 chunks of 20 ms, 40 ms each: about twice slower than real time
    let State::Running { rtf: Some(rtf), .. } = st.state else { panic!("no rtf") };
    assert!(rtf > 1.5 && rtf < 4.0, "rtf {rtf}");
    assert!(matches!(st.live_ok(), Err(LiveLimit::TooSlow { .. })));
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn a_crashed_server_is_started_again_after_a_wait() {
    let once = std::env::temp_dir().join(format!("kalango-die-once-{}", std::process::id()));
    let _ = std::fs::remove_file(&once);
    let cfg = config("crash", &[("FAKE_DIE_AFTER_BYTES", "20000"), ("FAKE_DIE_ONCE_FILE", once.to_str().unwrap())]);
    let service = Service::new(cfg);
    let _lease = service.lease();
    let first = running(&service).pid.unwrap();
    let mut stream = service.open_stream().unwrap();
    let _ = stream.push(&noise(8000, 1)); // 32000 bytes: the fake exits
    wait_for("the backoff", 5, || {
        matches!(service.status().state, State::Backoff { .. }) || service.status().starts == 2
    });
    wait_for("the restart", 10, || {
        service.status().starts == 2 && matches!(service.status().state, State::Running { .. })
    });
    let st = service.status();
    assert_ne!(st.pid.unwrap(), first);
    assert!(!alive(first));
    // the new server works
    let out = offline::isolate(&service, &noise(4800, 2), |_| true).expect("isolate after the restart");
    assert_eq!(out.samples.len(), 4800);
    drop(stream);
    service.shutdown(Duration::from_secs(5));
    let _ = std::fs::remove_file(once);
}

#[test]
fn it_gives_up_after_repeated_failures_and_retry_tries_again() {
    let mut cfg = config("giveup", &[]);
    cfg.server.server_bin = PathBuf::from("/nonexistent/audiocpp_server");
    cfg.max_attempts = 3;
    let service = Service::new(cfg);
    let _lease = service.lease();
    wait_for("giving up", 10, || matches!(service.status().state, State::Unavailable { .. }));
    let State::Unavailable { reason } = service.status().state else { unreachable!() };
    assert!(!reason.is_empty());
    assert!(matches!(service.wait_running(Duration::from_secs(1), &AtomicBool::new(false)), Err(Error::Protocol(_))));
    service.retry();
    wait_for("a new try", 5, || !matches!(service.status().state, State::Unavailable { .. }));
    wait_for("giving up again", 10, || matches!(service.status().state, State::Unavailable { .. }));
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn a_server_that_exits_at_once_reports_why() {
    // the fake without `--config` arguments panics: stands for a server that refuses its configuration
    let mut cfg = config("exits", &[]);
    cfg.server.server_bin = PathBuf::from("/bin/false");
    cfg.max_attempts = 1;
    let service = Service::new(cfg);
    let _lease = service.lease();
    wait_for("giving up", 10, || matches!(service.status().state, State::Unavailable { .. }));
    let State::Unavailable { reason } = service.status().state else { unreachable!() };
    assert!(reason.contains("stopped with code"), "{reason}");
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn offline_isolation_keeps_length_and_alignment() {
    let service = Service::new(config("offline", &[("FAKE_GAIN", "0.5"), ("FAKE_LAG", "2048")]));
    let _lease = service.lease();
    running(&service);
    let input = noise(SAMPLE_RATE as usize * 3 + 123, 3);
    let mut last = 0.0f32;
    let out = offline::isolate(&service, &input, |f| {
        assert!(f >= last, "progress only goes forward");
        last = f;
        true
    })
    .unwrap();
    assert_eq!(out.samples.len(), input.len());
    assert!((last - 1.0).abs() < 1e-6);
    let worst = out.samples.iter().zip(&input).map(|(o, i)| (o - 0.5 * i).abs()).fold(0.0f32, f32::max);
    assert!(worst < 2e-4, "sample for sample, 16-bit rounding only: {worst}");
    // a half-and-half mix
    let mixed = offline::blend(&input, &out.samples, 0.5);
    assert!((mixed[1000] - 0.75 * input[1000]).abs() < 2e-4);
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn offline_isolation_can_be_cancelled() {
    let service = Service::new(config("cancel", &[]));
    let _lease = service.lease();
    running(&service);
    let mut calls = 0;
    let r = offline::isolate(&service, &noise(SAMPLE_RATE as usize * 2, 4), |_| {
        calls += 1;
        calls < 3
    });
    assert!(matches!(r, Err(Error::Protocol(m)) if m == "cancelled"));
    // the server survives
    assert!(offline::isolate(&service, &noise(4800, 5), |_| true).is_ok());
    service.shutdown(Duration::from_secs(5));
}

/// Feeds `input` to `live` in 10 ms chunks paced like the audio engine and returns what came out.
fn run_live(live: &mut LiveIsolator, input: &[f32], amount: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    let mut next = Instant::now();
    for chunk in input.chunks(480) {
        let mut frames: Vec<[f32; 2]> = chunk.iter().map(|s| [*s, *s]).collect();
        live.process(&mut frames, amount);
        out.extend(frames.iter().map(|f| f[0]));
        next += Duration::from_millis(10);
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    out
}

/// The delay (in samples) at which `out[t] = gain * input[t - delay]` fits best over the last half, and the largest
/// error at that delay.
fn best_alignment(input: &[f32], out: &[f32], gain: f32) -> (usize, f32) {
    let from = out.len() / 2;
    let mut best = (0, f32::MAX);
    for delay in 0..12_000 {
        let mut err = 0.0f32;
        for t in from..out.len() - 1000 {
            err = err.max((out[t] - gain * input[t - delay]).abs());
            if err >= best.1 {
                break;
            }
        }
        if err < best.1 {
            best = (delay, err);
        }
    }
    best
}

fn wait_live(live: &LiveIsolator) {
    wait_for("the live stream", 10, || live.state() == LiveState::Running);
}

#[test]
fn live_wet_and_dry_line_up_so_the_mix_does_not_echo() {
    let service = Service::new(config("live", &[("FAKE_GAIN", "0.5"), ("FAKE_LAG", "2048")]));
    let mut live = LiveIsolator::new(service.clone());
    wait_live(&live);
    let input = noise(SAMPLE_RATE as usize * 3, 6);
    // only the isolated voice (gain 0.5 in the fake)
    let out = run_live(&mut live, &input, 1.0);
    let (delay, err) = best_alignment(&input, &out, 0.5);
    assert!(err < 3e-4, "isolated voice fits at delay {delay} with error {err}");
    // the latency the block reports is the delay the sound really has (one chunk of slack)
    let reported = live.latency_ms() * SAMPLE_RATE as f32 / 1000.0;
    assert!((reported - delay as f32).abs() <= 480.0, "reported {reported} samples, real {delay}");
    assert!(delay >= 2048, "at least the model's own delay: {delay}");
    // half and half: 0.5*0.5x + 0.5*x = 0.75x at the SAME delay (an echo would not fit)
    let out = run_live(&mut live, &input, 0.5);
    let (delay2, err2) = best_alignment(&input, &out, 0.75);
    assert!(err2 < 3e-4, "mix fits at delay {delay2} with error {err2}");
    // none of it: the original, delayed the same way
    let out = run_live(&mut live, &input, 0.0);
    let (_, err3) = best_alignment(&input, &out, 1.0);
    assert!(err3 < 1e-6, "{err3}");
    drop(live);
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn live_passes_the_sound_through_untouched_until_the_model_is_ready() {
    let service = Service::new(config("slowstart", &[("FAKE_START_DELAY_MS", "1500")]));
    let mut live = LiveIsolator::new(service.clone());
    let input = noise(4800, 7);
    let out = run_live(&mut live, &input, 1.0);
    assert!(matches!(live.state(), LiveState::Starting), "{:?}", live.state());
    assert_eq!(out, input, "starting: the original goes through unchanged");
    assert_eq!(live.latency_ms(), 0.0);
    wait_live(&live);
    drop(live);
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn live_on_a_cpu_only_server_is_offline_only_and_passes_through() {
    let service = Service::new(config("liveCpu", &[("FAKE_BACKEND", "cpu")]));
    let mut live = LiveIsolator::new(service.clone());
    wait_for("the verdict", 10, || matches!(live.state(), LiveState::OfflineOnly(_)));
    assert_eq!(live.state(), LiveState::OfflineOnly(LiveLimit::CpuOnly { rtf: None }));
    let input = noise(4800, 8);
    assert_eq!(run_live(&mut live, &input, 1.0), input);
    drop(live);
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn live_falls_back_when_the_model_cannot_keep_up() {
    // 20 ms of sound takes 60 ms: three times slower than real time (no probe, so live is tried anyway)
    let service = Service::new(config("liveSlow", &[("FAKE_MS_PER_CHUNK", "60")]));
    let mut live = LiveIsolator::new(service.clone());
    wait_live(&live);
    let input = noise(SAMPLE_RATE as usize * 3, 9);
    let out = run_live(&mut live, &input, 1.0);
    assert!(matches!(live.state(), LiveState::Fallback(_) | LiveState::Running), "{:?}", live.state());
    assert!(live.gaps() > 0 || live.state() != LiveState::Running, "falling behind is noticed");
    // once it gave up, the end of the take is the original, not silence
    let tail = &out[out.len() - 480..];
    let orig = &input[input.len() - 480..];
    assert!(tail == orig || live.state() == LiveState::Running, "after falling back the original passes through");
    drop(live);
    service.shutdown(Duration::from_secs(5));
}

#[test]
fn live_survives_a_server_crash_and_returns() {
    let once = std::env::temp_dir().join(format!("kalango-live-die-once-{}", std::process::id()));
    let _ = std::fs::remove_file(&once);
    // dies after 1 s of sound (192000 bytes would be the probe: it is off here)
    let cfg =
        config("liveCrash", &[("FAKE_DIE_AFTER_BYTES", "192000"), ("FAKE_DIE_ONCE_FILE", once.to_str().unwrap())]);
    let service = Service::new(cfg);
    let mut live = LiveIsolator::new(service.clone());
    wait_live(&live);
    let first = service.status().pid.unwrap();
    let input = noise(SAMPLE_RATE as usize * 2, 10);
    let out = run_live(&mut live, &input, 1.0);
    assert!(out.iter().all(|s| s.is_finite()));
    wait_for("a new server", 15, || {
        service.status().pid.is_some_and(|p| p != first) && live.state() == LiveState::Running
    });
    assert!(!alive(first));
    // working again
    let input = noise(SAMPLE_RATE as usize * 2, 11);
    let out = run_live(&mut live, &input, 1.0);
    let (_, err) = best_alignment(&input, &out, 0.5);
    assert!(err < 3e-4, "{err}");
    drop(live);
    service.shutdown(Duration::from_secs(5));
    let _ = std::fs::remove_file(once);
}

#[test]
fn dropping_the_live_isolator_lets_the_server_idle_out() {
    let mut cfg = config("liveIdle", &[]);
    cfg.idle_timeout = Duration::from_millis(300);
    let service = Service::new(cfg);
    let live = LiveIsolator::new(service.clone());
    wait_live(&live);
    let pid = service.status().pid.unwrap();
    drop(live);
    wait_for("idle", 5, || service.status().state == State::Idle);
    assert!(!alive(pid));
    service.shutdown(Duration::from_secs(5));
}
