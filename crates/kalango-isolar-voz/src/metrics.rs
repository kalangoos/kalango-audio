//! Comparison helpers for tests and benchmarks.

/// Scale-invariant signal-to-distortion ratio of `estimate` against `reference`, in dB.
pub fn si_sdr(estimate: &[f32], reference: &[f32]) -> f64 {
    let n = estimate.len().min(reference.len());
    let (e, r) = (&estimate[..n], &reference[..n]);
    let dot: f64 = e.iter().zip(r).map(|(a, b)| *a as f64 * *b as f64).sum();
    let rr: f64 = r.iter().map(|b| (*b as f64).powi(2)).sum();
    let alpha = dot / rr.max(1e-20);
    let (mut target, mut noise) = (0.0f64, 0.0f64);
    for (a, b) in e.iter().zip(r) {
        let t = alpha * *b as f64;
        target += t * t;
        noise += (*a as f64 - t).powi(2);
    }
    10.0 * (target.max(1e-20) / noise.max(1e-20)).log10()
}

/// The largest absolute difference between two signals.
pub fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// Reads a raw little-endian float32 file.
pub fn read_f32(path: &std::path::Path) -> std::io::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
}
