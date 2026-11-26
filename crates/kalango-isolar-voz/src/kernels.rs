//! Numeric kernels: the `x · wᵀ` product every linear layer runs on, and the element-wise functions.
//!
//! The product reads PyTorch's `[out, in]` weights straight from the memory-mapped checkpoint (no transposed copy):
//! each output is a dot product of an input row and a weight row, computed by a 4 × 3 register tile of AVX2 FMA
//! accumulators. CPUs without AVX2 + FMA take a portable path with the same results to rounding. Element-wise loops
//! are written once and compiled twice (`dispatch!`), so the AVX2 build vectorises them 8 lanes wide.

use std::sync::OnceLock;

pub fn has_avx2() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    })
}

/// Compiles an `#[inline(always)]` body twice: once with AVX2 + FMA enabled, once portable, and picks at run time.
macro_rules! dispatch {
    ($(#[$meta:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? => $body:ident) => {
        $(#[$meta])*
        $vis fn $name($($arg: $ty),*) $(-> $ret)? {
            #[cfg(target_arch = "x86_64")]
            {
                #[target_feature(enable = "avx2,fma")]
                unsafe fn fast($($arg: $ty),*) $(-> $ret)? { $body($($arg),*) }
                if $crate::kernels::has_avx2() {
                    // SAFETY: the features were detected at run time.
                    return unsafe { fast($($arg),*) };
                }
            }
            $body($($arg),*)
        }
    };
}
pub(crate) use dispatch;

/// `out[i·ldo + j] (+)= Σₖ x[i·ldx + k] · w[j·ldw + k] + bias[j]` for rows `i < m` and outputs `j` in `n0..n1`.
///
/// # Safety
/// `x` holds `m` rows of `k` values at stride `ldx`, `w` rows `n0..n1` of `k` values at stride `ldw`, `out` rows at
/// stride `ldo` with columns `n0..n1` writable and not written by anyone else meanwhile.
#[allow(clippy::too_many_arguments)]
pub unsafe fn matmul_nt(
    x: *const f32,
    ldx: usize,
    m: usize,
    k: usize,
    w: *const f32,
    ldw: usize,
    n0: usize,
    n1: usize,
    bias: Option<&[f32]>,
    out: *mut f32,
    ldo: usize,
    add: bool,
) {
    if m == 0 || n1 <= n0 {
        return;
    }
    let bias = bias.map_or(std::ptr::null(), |b| b.as_ptr());
    #[cfg(target_arch = "x86_64")]
    if has_avx2() {
        avx2::matmul(x, ldx, m, k, w, ldw, n0, n1, bias, out, ldo, add);
        return;
    }
    for i in 0..m {
        let row = std::slice::from_raw_parts(x.add(i * ldx), k);
        for j in n0..n1 {
            let weights = std::slice::from_raw_parts(w.add(j * ldw), k);
            let mut sum = 0.0f32;
            for (a, b) in row.iter().zip(weights) {
                sum += a * b;
            }
            if !bias.is_null() {
                sum += *bias.add(j);
            }
            let o = out.add(i * ldo + j);
            *o = if add { *o + sum } else { sum };
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod avx2 {
    use std::arch::x86_64::*;

    #[inline(always)]
    unsafe fn hsum(v: __m256) -> f32 {
        let lo = _mm256_castps256_ps128(v);
        let hi = _mm256_extractf128_ps(v, 1);
        let s = _mm_add_ps(lo, hi);
        let s = _mm_add_ps(s, _mm_movehl_ps(s, s));
        let s = _mm_add_ss(s, _mm_shuffle_ps(s, s, 1));
        _mm_cvtss_f32(s)
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
    unsafe fn tile<const MR: usize, const NR: usize>(
        x: *const f32,
        ldx: usize,
        k: usize,
        w: *const f32,
        ldw: usize,
        bias: *const f32,
        out: *mut f32,
        ldo: usize,
        add: bool,
    ) {
        let mut acc = [[_mm256_setzero_ps(); NR]; MR];
        let k8 = k & !7;
        let mut p = 0;
        while p < k8 {
            let mut wv = [_mm256_setzero_ps(); NR];
            for (j, wj) in wv.iter_mut().enumerate() {
                *wj = _mm256_loadu_ps(w.add(j * ldw + p));
            }
            for (i, row) in acc.iter_mut().enumerate() {
                let xv = _mm256_loadu_ps(x.add(i * ldx + p));
                for (a, wj) in row.iter_mut().zip(&wv) {
                    *a = _mm256_fmadd_ps(xv, *wj, *a);
                }
            }
            p += 8;
        }
        for i in 0..MR {
            for j in 0..NR {
                let mut s = hsum(acc[i][j]);
                for q in k8..k {
                    s += *x.add(i * ldx + q) * *w.add(j * ldw + q);
                }
                if !bias.is_null() {
                    s += *bias.add(j);
                }
                let o = out.add(i * ldo + j);
                *o = if add { *o + s } else { s };
            }
        }
    }

    #[target_feature(enable = "avx2,fma")]
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn matmul(
        x: *const f32,
        ldx: usize,
        m: usize,
        k: usize,
        w: *const f32,
        ldw: usize,
        n0: usize,
        n1: usize,
        bias: *const f32,
        out: *mut f32,
        ldo: usize,
        add: bool,
    ) {
        // weight rows per block ~96 KiB (they stay in L2 while the input rows stream past), input rows per block
        // ~192 KiB
        let nb = ((24 * 1024 / k.max(1)) / 3 * 3).max(3);
        let mb = ((48 * 1024 / k.max(1)) / 4 * 4).max(4);
        let mut jb = n0;
        while jb < n1 {
            let je = (jb + nb).min(n1);
            let mut ib = 0;
            while ib < m {
                let ie = (ib + mb).min(m);
                let mut j = jb;
                while j < je {
                    let nr = (je - j).min(3);
                    let wj = w.add(j * ldw);
                    let bj = if bias.is_null() { bias } else { bias.add(j) };
                    let mut i = ib;
                    while i < ie {
                        let mr = (ie - i).min(4);
                        let xi = x.add(i * ldx);
                        let oi = out.add(i * ldo + j);
                        macro_rules! go {
                            ($a:literal, $b:literal) => {
                                tile::<$a, $b>(xi, ldx, k, wj, ldw, bj, oi, ldo, add)
                            };
                        }
                        match (mr, nr) {
                            (4, 3) => go!(4, 3),
                            (4, 2) => go!(4, 2),
                            (4, 1) => go!(4, 1),
                            (3, 3) => go!(3, 3),
                            (3, 2) => go!(3, 2),
                            (3, 1) => go!(3, 1),
                            (2, 3) => go!(2, 3),
                            (2, 2) => go!(2, 2),
                            (2, 1) => go!(2, 1),
                            (1, 3) => go!(1, 3),
                            (1, 2) => go!(1, 2),
                            _ => go!(1, 1),
                        }
                        i += mr;
                    }
                    j += nr;
                }
                ib = ie;
            }
            jb = je;
        }
    }
}

/// e^x to ~2 ulp (Cephes' polynomial), written so loops over it vectorise.
#[inline(always)]
#[allow(clippy::excessive_precision)]
pub fn exp(x: f32) -> f32 {
    let x = x.clamp(-87.33654, 88.37626);
    let fx = (x * std::f32::consts::LOG2_E + 0.5).floor();
    let r = x - fx * 0.693_359_4 - fx * -2.121_944_4e-4;
    let mut y = 1.987_569_1e-4f32;
    y = y * r + 1.398_199_9e-3;
    y = y * r + 8.333_452e-3;
    y = y * r + 4.166_579_6e-2;
    y = y * r + 1.666_666_5e-1;
    y = y * r + 5.000_000_1e-1;
    y = y * r * r + r + 1.0;
    y * f32::from_bits(((fx as i32 + 127) << 23) as u32)
}

/// erf(x) to 1.5e-7 (Abramowitz & Stegun 7.1.26).
#[inline(always)]
pub fn erf(x: f32) -> f32 {
    let a = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * a);
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152 + t * 1.061_405_4))));
    let e = 1.0 - poly * exp(-a * a);
    if x < 0.0 {
        -e
    } else {
        e
    }
}

#[inline(always)]
fn gelu_body(values: &mut [f32]) {
    for v in values.iter_mut() {
        *v = 0.5 * *v * (1.0 + erf(*v * std::f32::consts::FRAC_1_SQRT_2));
    }
}

dispatch! {
    /// GELU with the exact (erf) form PyTorch's `nn.GELU()` uses, in place.
    pub fn gelu(values: &mut [f32]) => gelu_body
}

#[inline(always)]
fn layer_norm_body(x: &[f32], width: usize, gain: &[f32], bias: &[f32], out: &mut [f32]) {
    for (row, dst) in x.chunks_exact(width).zip(out.chunks_exact_mut(width)) {
        let mut sum = 0.0f32;
        for v in row {
            sum += v;
        }
        let mean = sum / width as f32;
        let mut var = 0.0f32;
        for v in row {
            let d = v - mean;
            var += d * d;
        }
        let inv = 1.0 / (var / width as f32 + 1e-5).sqrt();
        for i in 0..width {
            dst[i] = (row[i] - mean) * inv * gain[i] + bias[i];
        }
    }
}

dispatch! {
    /// Layer norm (eps 1e-5) of every `width`-long row of `x` into `out`.
    pub fn layer_norm(x: &[f32], width: usize, gain: &[f32], bias: &[f32], out: &mut [f32]) => layer_norm_body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matmul_matches_naive() {
        for &(m, k, n) in &[(1, 8, 1), (5, 13, 7), (92, 512, 40), (7, 2, 5), (33, 1024, 17)] {
            let x: Vec<f32> = (0..m * k).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
            let w: Vec<f32> = (0..n * k).map(|i| ((i * 53 % 97) as f32 - 48.0) / 48.0).collect();
            let b: Vec<f32> = (0..n).map(|i| i as f32 * 0.1).collect();
            let mut out = vec![1.0f32; m * n];
            unsafe { matmul_nt(x.as_ptr(), k, m, k, w.as_ptr(), k, 0, n, Some(&b), out.as_mut_ptr(), n, true) };
            for i in 0..m {
                for j in 0..n {
                    let mut s = 1.0 + b[j];
                    for p in 0..k {
                        s += x[i * k + p] * w[j * k + p];
                    }
                    assert!((s - out[i * n + j]).abs() < 1e-3 * (1.0 + s.abs()), "{m}x{k}x{n} at {i},{j}");
                }
            }
        }
    }

    #[test]
    fn exp_and_erf_are_accurate() {
        for i in -2000..2000 {
            let x = i as f32 * 0.01;
            assert!((exp(x) - x.exp()).abs() <= 3e-7 * x.exp().max(1e-30));
            let e = libm_erf(x as f64);
            assert!((erf(x) as f64 - e).abs() < 3e-7, "erf({x})");
        }
    }

    fn libm_erf(x: f64) -> f64 {
        // series / continued fraction good to ~1e-12 on [-20, 20]
        let t = 1.0 / (1.0 + 0.5 * x.abs());
        let y = 1.0
            - t * (-x * x - 1.265_512_23
                + t * (1.000_023_68
                    + t * (0.374_091_96
                        + t * (0.096_784_18
                            + t * (-0.186_288_06
                                + t * (0.278_868_07
                                    + t * (-1.135_203_98
                                        + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
                .exp();
        if x >= 0.0 {
            y
        } else {
            -y
        }
    }
}
