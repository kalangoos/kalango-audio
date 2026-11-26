//! Streams a raw f32 48 kHz mono clip through a real `audiocpp_server` in lock step and reports speed and latency.
//!
//! ```text
//! cargo run --release -p kalango-audiocpp --example stream -- <server_bin> <model> <cuda|vulkan|cpu> <in.f32> <out.f32> \
//!     [seconds=clip length] [block_samples=960] [hold_samples=2944]
//! ```
//! Per block: push `block_samples`, then pull until the output count reaches `pushed - hold_samples` (the algorithmic
//! delay: 2048 + 480 x look-ahead). The time that takes is the per-block latency reported (p50/p99/max). RTF is wall
//! time (first push to last output) over audio duration. Input is repeated to reach `seconds`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kalango_audiocpp::{Backend, Config, Server, SAMPLE_RATE};

fn read_f32(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("input");
    bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn rss_of(pid: u32, field: &str) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(field))
        .map(|l| l[field.len() + 1..].trim().to_string())
        .unwrap_or_default()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let backend = match a[2].as_str() {
        "cuda" => Backend::Cuda,
        "vulkan" => Backend::Vulkan,
        _ => Backend::Cpu,
    };
    let clip = read_f32(&a[3]);
    let seconds: f64 = a.get(5).and_then(|v| v.parse().ok()).unwrap_or(clip.len() as f64 / SAMPLE_RATE as f64);
    let block: usize = a.get(6).and_then(|v| v.parse().ok()).unwrap_or(960);
    let hold: usize = a.get(7).and_then(|v| v.parse().ok()).unwrap_or(2944);
    let input: Vec<f32> = clip.iter().copied().cycle().take((seconds * SAMPLE_RATE as f64) as usize).collect();

    let mut cfg =
        Config::new(PathBuf::from(&a[0]), PathBuf::from(&a[1]), std::env::temp_dir().join("kalango-audiocpp-stream"));
    cfg.backend = backend;
    cfg.threads = 4;
    let t0 = Instant::now();
    let server = Server::spawn(&cfg).expect("spawn");
    let startup = t0.elapsed().as_secs_f64();
    let pid = server.pid();
    let mut stream = server.open_stream().expect("stream");
    let mut output: Vec<f32> = Vec::with_capacity(input.len());
    let mut lat = Vec::new();
    let start = Instant::now();
    let mut pushed = 0usize;
    for piece in input.chunks(block) {
        let t = Instant::now();
        stream.push(piece).expect("push");
        pushed += piece.len();
        while output.len() + hold < pushed {
            match stream.pull(Duration::from_secs(5)).expect("pull") {
                Some(part) if !part.is_empty() => output.extend(part),
                Some(_) => panic!("no output for 5 s at {} samples", output.len()),
                None => panic!("stream closed early"),
            }
        }
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    output.extend(stream.finish().expect("finish"));
    let total = start.elapsed().as_secs_f64();
    let peak_rss = rss_of(pid, "VmHWM");
    lat.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let steady = &lat[(lat.len() / 50).min(lat.len() - 1)..];
    println!(
        "backend={} audio={seconds:.1}s block={block} startup={startup:.2}s RTF={:.3} block p50={:.2}ms p99={:.2}ms max={:.2}ms out_samples={} in_samples={} server_VmHWM={peak_rss}",
        a[2],
        total / seconds,
        percentile(steady, 0.5),
        percentile(steady, 0.99),
        steady[steady.len() - 1],
        output.len(),
        input.len(),
    );
    let bytes: Vec<u8> = output.iter().flat_map(|s| s.to_le_bytes()).collect();
    std::fs::write(&a[4], bytes).expect("write output");
    drop(stream);
    println!("server exit code: {}", server.stop(Duration::from_secs(5)).expect("stop"));
}
