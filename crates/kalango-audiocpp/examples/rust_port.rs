//! Runs a clip through our own Rust port (the parity/audit reference) and writes raw f32, to compare with audio.cpp.
//!
//! ```text
//! cargo run --release -p kalango-audiocpp --example rust_port -- <realtime|offline> <checkpoint.safetensors> <in.f32> <out.f32> [threads=8]
//! ```

use std::path::Path;

use kalango_isolar_voz::metrics::read_f32;
use kalango_isolar_voz::{Offline, Realtime, RealtimeOptions};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let threads: usize = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(8);
    let input = read_f32(Path::new(&a[2])).expect("input");
    let started = std::time::Instant::now();
    let output = if a[0] == "offline" {
        Offline::open(Path::new(&a[1]), threads).expect("open").process(&input)
    } else {
        let mut rt = Realtime::open(Path::new(&a[1]), RealtimeOptions { threads, ..Default::default() }).expect("open");
        let mut out = Vec::new();
        for piece in input.chunks(960) {
            rt.push(piece, &mut out);
        }
        rt.flush(&mut out);
        out
    };
    let bytes: Vec<u8> = output.iter().flat_map(|s| s.to_le_bytes()).collect();
    std::fs::write(&a[3], bytes).expect("write");
    println!("{} samples in {:.2} s", output.len(), started.elapsed().as_secs_f64());
}
