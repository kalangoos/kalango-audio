//! Streams a raw f32 48 kHz mono clip through a real `audiocpp_server` and reports speed and latency.
//!
//! ```text
//! cargo run --release -p kalango-audiocpp --example stream -- <server_bin> <model> <cuda|vulkan|cpu> <in.f32> <out.f32> \
//!     [seconds=clip length] [block_samples=960] [hold_samples=2944] [pace]
//! ```
//! Input is repeated to reach `seconds`. Without `pace` blocks are pushed as fast as possible (RTF = wall time over
//! audio time). With `pace` one block is pushed per block duration (a live microphone). The latency of an output piece
//! is its arrival time minus the push time of the input block it needed (output index + `hold_samples`, the algorithmic
//! delay 2048 + 480 x look-ahead); unpaced runs measure queueing, so use `pace` for latency.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kalango_audiocpp::{Backend, Config, Server, Stream, SAMPLE_RATE};

fn read_f32(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("input");
    bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
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

/// Collects output that is ready within `wait`; `false` when the server closed the stream.
fn take(stream: &mut Stream, wait: Duration, output: &mut Vec<f32>, arrivals: &mut Vec<(Instant, usize)>) -> bool {
    match stream.pull(wait).expect("pull") {
        Some(part) => {
            if !part.is_empty() {
                output.extend(part);
                arrivals.push((Instant::now(), output.len()));
            }
            true
        }
        None => false,
    }
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
    let pace = a.get(8).is_some_and(|v| v == "pace");
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
    let mut arrivals: Vec<(Instant, usize)> = Vec::new();
    let mut pushes: Vec<Instant> = Vec::new();
    let start = Instant::now();
    let period = Duration::from_secs_f64(block as f64 / SAMPLE_RATE as f64);
    for (k, piece) in input.chunks(block).enumerate() {
        if pace {
            // wait for this block's slot, collecting output that arrives meanwhile
            let slot = start + period * k as u32;
            while Instant::now() < slot {
                assert!(take(&mut stream, slot - Instant::now(), &mut output, &mut arrivals), "stream closed early");
            }
        }
        pushes.push(Instant::now());
        stream.push(piece).expect("push");
        loop {
            let before = output.len();
            take(&mut stream, Duration::ZERO, &mut output, &mut arrivals);
            if output.len() == before {
                break;
            }
        }
    }
    let end_of_input = Instant::now();
    let tail = stream.finish().expect("finish");
    output.extend(tail);
    let total = start.elapsed().as_secs_f64();
    let peak_rss = rss_of(pid, "VmHWM");
    let mut lat: Vec<f64> = arrivals
        .iter()
        .filter(|(t, _)| *t < end_of_input)
        .map(|&(t, n)| {
            let needed = ((n + hold).saturating_sub(1) / block).min(pushes.len() - 1);
            t.saturating_duration_since(pushes[needed]).as_secs_f64() * 1000.0
        })
        .collect();
    lat.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let steady = &lat[(lat.len() / 50).min(lat.len().saturating_sub(1))..];
    println!(
        "backend={} paced={pace} audio={seconds:.1}s block={block} startup={startup:.2}s wall={total:.2}s RTF={:.3} latency p50={:.2}ms p99={:.2}ms max={:.2}ms pieces={} out_samples={} in_samples={} server_VmHWM={peak_rss}",
        a[2],
        total / seconds,
        percentile(steady, 0.5),
        percentile(steady, 0.99),
        steady.last().copied().unwrap_or(f64::NAN),
        lat.len(),
        output.len(),
        input.len(),
    );
    let bytes: Vec<u8> = output.iter().flat_map(|s| s.to_le_bytes()).collect();
    std::fs::write(&a[4], bytes).expect("write output");
    drop(stream);
    println!("server exit code: {}", server.stop(Duration::from_secs(5)).expect("stop"));
}
