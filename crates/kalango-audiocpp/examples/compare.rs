//! Compares two raw f32 clips: SI-SDR of `ours` against `reference`, and the largest absolute difference.
//!
//! ```text
//! cargo run --release -p kalango-audiocpp --example compare -- <ours.f32> <reference.f32> [shift_samples=0]
//! ```
//! A positive shift drops that many leading samples of `ours` first (use it to test for a constant delay).

use kalango_isolar_voz::metrics::{max_abs_diff, read_f32, si_sdr};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let ours = read_f32(std::path::Path::new(&a[0])).expect("ours");
    let reference = read_f32(std::path::Path::new(&a[1])).expect("reference");
    let shift: usize = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
    let ours = &ours[shift.min(ours.len())..];
    let n = ours.len().min(reference.len());
    println!(
        "ours={} reference={} compared={n} SI-SDR={:.2} dB max_abs_err={:.3e}",
        ours.len(),
        reference.len(),
        si_sdr(&ours[..n], &reference[..n]),
        max_abs_diff(&ours[..n], &reference[..n]),
    );
}
