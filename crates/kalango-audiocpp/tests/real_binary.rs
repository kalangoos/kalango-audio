//! Tests against the real `audiocpp_server`. Ignored by default; run with
//!
//! ```text
//! KALANGO_AUDIOCPP_SERVER=/abs/path/audiocpp_server KALANGO_AUDIOCPP_MODEL=/abs/path/voice-isolator-realtime-f32.gguf \
//!   [KALANGO_AUDIOCPP_BACKEND=cuda|vulkan|cpu] cargo test -p kalango-audiocpp -- --ignored
//! ```
//! Without the variables the tests print why they skip and pass.

use std::path::PathBuf;
use std::time::Duration;

use kalango_audiocpp::{Backend, Config, Error, Server, SAMPLE_RATE};

fn config(tag: &str) -> Option<Config> {
    let server = std::env::var_os("KALANGO_AUDIOCPP_SERVER").map(PathBuf::from);
    let model = std::env::var_os("KALANGO_AUDIOCPP_MODEL").map(PathBuf::from);
    let (Some(server), Some(model)) = (server, model) else {
        eprintln!("skipped: set KALANGO_AUDIOCPP_SERVER and KALANGO_AUDIOCPP_MODEL");
        return None;
    };
    let mut cfg = Config::new(server, model, std::env::temp_dir().join(format!("kalango-audiocpp-test-{tag}")));
    cfg.backend = match std::env::var("KALANGO_AUDIOCPP_BACKEND").as_deref() {
        Ok("vulkan") => Backend::Vulkan,
        Ok("cpu") => Backend::Cpu,
        _ => Backend::Cuda,
    };
    cfg.threads = 4;
    cfg.startup_timeout = Duration::from_secs(120);
    Some(cfg)
}

fn alive(pid: u32) -> bool {
    // a reaped child has no /proc entry; a zombie would still show here
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok()
}

#[test]
#[ignore]
fn spawn_health_stop() {
    let Some(cfg) = config("health") else { return };
    let mut server = Server::spawn(&cfg).expect("spawn");
    server.health().expect("health");
    assert!(server.is_running());
    let pid = server.pid();
    let code = server.stop(Duration::from_secs(5)).expect("stop");
    assert!(code == 0 || code == 128 + 15, "unexpected exit code {code}");
    assert!(!alive(pid), "the process must be reaped");
}

#[test]
#[ignore]
fn drop_kills_the_process() {
    let Some(cfg) = config("drop") else { return };
    let server = Server::spawn(&cfg).expect("spawn");
    let pid = server.pid();
    assert!(alive(pid));
    drop(server);
    assert!(!alive(pid), "kill-on-drop must reap the child");
}

#[test]
#[ignore]
fn a_bad_model_path_fails_to_start() {
    let Some(mut cfg) = config("bad") else { return };
    cfg.model = PathBuf::from("/nonexistent/model.gguf");
    cfg.startup_timeout = Duration::from_secs(30);
    match Server::spawn(&cfg) {
        Err(Error::ServerExited(_)) | Err(Error::Timeout(_)) => {}
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("a missing model must not become healthy"),
    }
}

#[test]
#[ignore]
fn stream_returns_isolated_audio_of_the_same_length() {
    let Some(cfg) = config("stream") else { return };
    let server = Server::spawn(&cfg).expect("spawn");
    // 2 s of a 220 Hz tone plus a little deterministic noise
    let mut state = 1u32;
    let input: Vec<f32> = (0..2 * SAMPLE_RATE as usize)
        .map(|i| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            0.3 * (i as f32 * 220.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() + 0.05 * noise
        })
        .collect();
    let mut stream = server.open_stream().expect("stream");
    let mut out = Vec::new();
    for block in input.chunks(960) {
        stream.push(block).expect("push");
        if let Some(part) = stream.pull(Duration::from_millis(1)).expect("pull") {
            out.extend(part);
        }
    }
    out.extend(stream.finish().expect("finish"));
    assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    let diff = out.len().abs_diff(input.len());
    assert!(diff <= 4096, "output {} vs input {} samples", out.len(), input.len());
    assert!(out.iter().any(|s| s.abs() > 1e-4), "the output must not be silent");
    server.health().expect("the server must survive the stream");
}
