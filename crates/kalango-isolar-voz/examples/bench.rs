//! Speed and memory on this machine, with the real weights.
//!
//! ```text
//! cargo run --release -p kalango-isolar-voz --example bench -- realtime <threads> [cache_frames] [seconds]
//! cargo run --release -p kalango-isolar-voz --example bench -- offline <threads> [seconds]
//! ```
//! Input: the 12 s reference clip (`KALANGO_ISOLAR_VOZ_DIR/ref/pink_5db_12s.in.f32`), repeated to `seconds`.

use std::path::PathBuf;
use std::time::Instant;

use kalango_isolar_voz::metrics::read_f32;
use kalango_isolar_voz::{Offline, Realtime, RealtimeOptions, OFFLINE_FILE, REALTIME_FILE, SAMPLE_RATE};

fn status(field: &str) -> String {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(field))
        .map(|l| l[field.len() + 1..].trim().to_string())
        .unwrap_or_default()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = std::env::var_os("KALANGO_ISOLAR_VOZ_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/kalango-isolar-voz")
    });
    let mode = args.first().map(String::as_str).unwrap_or("realtime");
    let threads: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(1);
    let clip = read_f32(&base.join("ref/pink_5db_12s.in.f32")).expect("clip de referência");
    let rate = SAMPLE_RATE as f64;
    match mode {
        "realtime" => {
            let cache: Option<usize> = args.get(2).and_then(|v| v.parse().ok()).filter(|&c| c > 0);
            let seconds: f64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(12.0);
            let input: Vec<f32> = clip.iter().copied().cycle().take((seconds * rate) as usize).collect();
            let t0 = Instant::now();
            let options = RealtimeOptions { threads, cache_frames: cache, ..Default::default() };
            let mut rt = Realtime::open(&base.join("weights").join(REALTIME_FILE), options).unwrap();
            let load = t0.elapsed().as_secs_f64();
            let mut out = vec![0f32; 960];
            let mut times = Vec::with_capacity(input.len() / 960);
            let start = Instant::now();
            for piece in input.as_chunks::<960>().0 {
                let t = Instant::now();
                rt.push_frame(piece, &mut out);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let total = start.elapsed().as_secs_f64();
            // the first pushes only fill the analysis window; the steady state is what a callback sees
            let mut steady = times[4..].to_vec();
            steady.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "realtime threads={threads} cache={} audio={seconds:.1}s load={load:.3}s RTF={:.3} push20ms p50={:.2}ms p99={:.2}ms max={:.2}ms latency={}samples state={:.0}MiB RssAnon={} VmHWM={}",
                cache.map_or("janela".to_string(), |c| c.to_string()),
                total / seconds,
                percentile(&steady, 0.5),
                percentile(&steady, 0.99),
                steady.last().unwrap(),
                rt.frame_latency(960),
                rt.state_bytes() as f64 / (1 << 20) as f64,
                status("RssAnon"),
                status("VmHWM"),
            );
        }
        "offline" => {
            let seconds: f64 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(12.0);
            let input: Vec<f32> = clip.iter().copied().cycle().take((seconds * rate) as usize).collect();
            let mut model = Offline::open(&base.join("weights").join(OFFLINE_FILE), threads).unwrap();
            let start = Instant::now();
            let out = model.process(&input);
            let total = start.elapsed().as_secs_f64();
            assert_eq!(out.len(), input.len());
            println!(
                "offline threads={threads} audio={seconds:.1}s time={total:.2}s RTF={:.3} RssAnon={} VmHWM={}",
                total / seconds,
                status("RssAnon"),
                status("VmHWM"),
            );
        }
        _ => eprintln!("uso: bench realtime|offline <threads> ..."),
    }
}
