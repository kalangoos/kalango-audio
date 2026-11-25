//! The forward pass: attention, Mamba, the block loop, encoder, heads, correction and the STFT.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::*;

/// Bands per time-axis group of a clip of `frames` frames.
fn time_group(bands: usize, frames: usize) -> usize {
    (GROUP_ROWS / frames.max(1)).clamp(1, bands)
}

/// Frames per band-axis group of a clip of `frames` frames.
fn band_group(bands: usize, frames: usize) -> usize {
    (GROUP_ROWS / bands.max(1)).clamp(1, frames.max(1))
}

/// `src` `[a][b][w]` into `dst` `[b][a][w]`.
fn transpose(pool: &Pool, src: &[f32], dst: &mut [f32], a: usize, b: usize, w: usize) {
    let d = SharedMut::new(dst);
    pool.run(a, |i, _| {
        for j in 0..b {
            unsafe { d.slice((j * a + i) * w, w) }.copy_from_slice(&src[(i * b + j) * w..(i * b + j + 1) * w]);
        }
    });
}

fn add_into(pool: &Pool, dst: &mut [f32], src: &[f32]) {
    let n = src.len();
    let d = SharedMut::new(dst);
    let chunk = 1 << 15;
    pool.run(n.div_ceil(chunk), |task, _| {
        let (s, e) = (task * chunk, ((task + 1) * chunk).min(n));
        for (a, b) in unsafe { d.slice(s, e - s) }.iter_mut().zip(&src[s..e]) {
            *a += b;
        }
    });
}

fn par_gelu(pool: &Pool, x: &mut [f32]) {
    let n = x.len();
    let d = SharedMut::new(x);
    let chunk = 1 << 14;
    pool.run(n.div_ceil(chunk), |task, _| {
        let (s, e) = (task * chunk, ((task + 1) * chunk).min(n));
        kernels::gelu(unsafe { d.slice(s, e - s) });
    });
}

struct MambaBufs<'a> {
    xz: &'a mut [f32],
    u: &'a mut [f32],
    dbc: &'a mut [f32],
    delta: &'a mut [f32],
    y: &'a mut [f32],
    h: &'a mut [f32],
}

/// One sequence's attention for one head, all of its keys in the same `qkv` rows.
struct SeqAttn {
    qkv: *const f32,
    len: usize,
    head: usize,
    hd: usize,
    width: usize,
    causal: bool,
    window: usize,
    exclusive: bool,
    scratch: *mut f32,
    out: *mut f32,
}

#[inline(always)]
fn softmax_row(row: &mut [f32], scale: f32) {
    let mut max = f32::NEG_INFINITY;
    for &v in row.iter() {
        max = max.max(v);
    }
    let mut sum = 0.0;
    for v in row.iter_mut() {
        *v = exp((*v - max) * scale);
        sum += *v;
    }
    let inv = 1.0 / sum;
    for v in row.iter_mut() {
        *v *= inv;
    }
}

/// Exclusive self attention: removes the output's component along the token's own value.
#[inline(always)]
fn exclusive_body(o: &mut [f32], v: &[f32]) {
    let (mut ov, mut vv) = (0.0f32, 0.0f32);
    for (a, b) in o.iter().zip(v) {
        ov += a * b;
        vv += b * b;
    }
    let coef = ov / vv.max(1e-6);
    for (a, b) in o.iter_mut().zip(v) {
        *a -= coef * b;
    }
}

#[inline(always)]
fn seq_attn_body(a: &SeqAttn) {
    unsafe {
        let (len, hd, w) = (a.len, a.hd, a.width);
        let ld = 3 * w;
        let q = a.qkv.add(a.head * hd);
        let k = a.qkv.add(w + a.head * hd);
        let v = a.qkv.add(2 * w + a.head * hd);
        let scores = std::slice::from_raw_parts_mut(a.scratch, len * len);
        let vt = std::slice::from_raw_parts_mut(a.scratch.add(len * len), hd * len);
        let ot = std::slice::from_raw_parts_mut(a.scratch.add(len * len + hd * len), len * hd);
        matmul_nt(q, ld, len, hd, k, ld, 0, len, None, scores.as_mut_ptr(), len, false);
        let scale = 1.0 / (hd as f32).sqrt();
        for i in 0..len {
            let row = &mut scores[i * len..(i + 1) * len];
            let (lo, hi) = if a.causal {
                (if a.window > 0 { (i + 1).saturating_sub(a.window) } else { 0 }, i + 1)
            } else {
                (0, len)
            };
            softmax_row(&mut row[lo..hi], scale);
            row[..lo].fill(0.0);
            row[hi..].fill(0.0);
        }
        for j in 0..len {
            for d in 0..hd {
                vt[d * len + j] = *v.add(j * ld + d);
            }
        }
        matmul_nt(scores.as_ptr(), len, len, len, vt.as_ptr(), len, 0, hd, None, ot.as_mut_ptr(), hd, false);
        for i in 0..len {
            let o = &mut ot[i * hd..(i + 1) * hd];
            if a.exclusive {
                exclusive_body(o, std::slice::from_raw_parts(v.add(i * ld), hd));
            }
            std::slice::from_raw_parts_mut(a.out.add(i * w + a.head * hd), hd).copy_from_slice(o);
        }
    }
}

dispatch! {
    fn seq_attn(a: &SeqAttn) => seq_attn_body
}

/// One band's attention for one head over a stream's new frames, against its ring of past keys and values.
struct RingAttn {
    qkv: *const f32,
    frames: usize,
    pos0: usize,
    head: usize,
    hd: usize,
    width: usize,
    window: usize,
    cache_frames: usize,
    capacity: usize,
    exclusive: bool,
    keys: *mut f32,
    values: *mut f32,
    scratch: *mut f32,
    out: *mut f32,
}

#[inline(always)]
fn ring_attn_body(a: &RingAttn) {
    unsafe {
        let (hd, w, cap) = (a.hd, a.width, a.capacity);
        let ld = 3 * w;
        for t in 0..a.frames {
            let slot = (a.pos0 + t) % cap;
            let row = a.qkv.add(t * ld);
            std::ptr::copy_nonoverlapping(row.add(w + a.head * hd), a.keys.add(slot * hd), hd);
            std::ptr::copy_nonoverlapping(row.add(2 * w + a.head * hd), a.values.add(slot * hd), hd);
        }
        let scale = 1.0 / (hd as f32).sqrt();
        // what the frames before this block may contribute: the last `cache_frames` of them
        let floor = a.pos0.saturating_sub(a.cache_frames);
        for t in 0..a.frames {
            let q = a.pos0 + t;
            let mut lo = floor;
            if a.window > 0 {
                lo = lo.max((q + 1).saturating_sub(a.window));
            }
            let n = q + 1 - lo;
            let scores = std::slice::from_raw_parts_mut(a.scratch, n);
            let qv = std::slice::from_raw_parts(a.qkv.add(t * ld + a.head * hd), hd);
            let mut slot = lo % cap;
            for s in scores.iter_mut() {
                let kv = std::slice::from_raw_parts(a.keys.add(slot * hd), hd);
                let mut dot = 0.0f32;
                for d in 0..hd {
                    dot += qv[d] * kv[d];
                }
                *s = dot;
                slot += 1;
                if slot == cap {
                    slot = 0;
                }
            }
            softmax_row(scores, scale);
            let o = std::slice::from_raw_parts_mut(a.scratch.add(n), hd);
            o.fill(0.0);
            let mut slot = lo % cap;
            for &p in scores.iter() {
                let vv = std::slice::from_raw_parts(a.values.add(slot * hd), hd);
                for d in 0..hd {
                    o[d] += p * vv[d];
                }
                slot += 1;
                if slot == cap {
                    slot = 0;
                }
            }
            if a.exclusive {
                exclusive_body(o, std::slice::from_raw_parts(a.qkv.add(t * ld + 2 * w + a.head * hd), hd));
            }
            std::slice::from_raw_parts_mut(a.out.add(t * w + a.head * hd), hd).copy_from_slice(o);
        }
    }
}

dispatch! {
    fn ring_attn(a: &RingAttn) => ring_attn_body
}

/// A stream's view of one attention block's ring: keys, values, the position of the block's first frame, the frames
/// before it that may be seen, the ring's slots.
type Ring<'a> = (&'a mut [f32], &'a mut [f32], usize, usize, usize);

impl Network {
    /// One Mamba direction over `nseq` sequences of `len` rows of `x` into `out`. `state`: each sequence's carried
    /// convolution tail and recurrence (a stream's time axis); without it both start from zeros.
    #[allow(clippy::too_many_arguments)]
    fn mamba(
        &self,
        pool: &Pool,
        layer: &MambaLayer,
        x: &[f32],
        nseq: usize,
        len: usize,
        reverse: bool,
        state: Option<(&mut [f32], &mut [f32])>,
        b: &mut MambaBufs,
        out: &mut [f32],
    ) {
        let w = self.cfg.width;
        let inner = 2 * w;
        let rows = nseq * len;
        let dt_rank = layer.dt_proj.k;
        let dbc_ld = layer.x_proj.n;
        layer.in_proj.apply(pool, x, w, rows, b.xz, 2 * inner, false);
        let (tail, h) = match state {
            Some((t, h)) => (t.as_mut_ptr(), h.as_mut_ptr()),
            None => {
                b.h[..nseq * inner * MAMBA_STATE].fill(0.0);
                (std::ptr::null_mut(), b.h.as_mut_ptr())
            }
        };
        let (tail, h) = (SharedMut(tail), SharedMut(h));
        let chunks = inner / 128;
        let xz = b.xz.as_ptr() as usize;
        let u = SharedMut::new(b.u);
        pool.run(nseq * chunks, |task, _| {
            let (seq, chunk) = (task / chunks, task % chunks);
            conv(&ConvArgs {
                rows: len,
                reverse,
                c0: chunk * 128,
                c1: chunk * 128 + 128,
                inner,
                xz: (xz as *const f32).wrapping_add(seq * len * 2 * inner),
                w: layer.conv_w.as_ptr(),
                b: layer.conv_b.as_ptr(),
                tail: if tail.ptr().is_null() { tail.ptr() } else { tail.ptr().wrapping_add(seq * (MAMBA_CONV - 1) * inner) },
                out: u.ptr().wrapping_add(seq * len * inner),
            });
        });
        layer.x_proj.apply(pool, b.u, inner, rows, b.dbc, dbc_ld, false);
        layer.dt_proj.apply(pool, b.dbc, dbc_ld, rows, b.delta, inner, false);
        let delta = SharedMut::new(b.delta);
        par_rows(pool, rows, 32, |r0, r1| softplus(unsafe { delta.slice(r0 * inner, (r1 - r0) * inner) }));
        let per_task = 64;
        let tasks = inner / per_task;
        let (u, dbc, y) = (b.u.as_ptr() as usize, b.dbc.as_ptr() as usize, SharedMut::new(b.y));
        pool.run(nseq * tasks, |task, _| {
            let (seq, chunk) = (task / tasks, task % tasks);
            let off = seq * len;
            scan(&ScanArgs {
                rows: len,
                reverse,
                c0: chunk * per_task,
                c1: chunk * per_task + per_task,
                inner,
                dbc_ld,
                dt_rank,
                u: (u as *const f32).wrapping_add(off * inner),
                delta: (delta.ptr() as *const f32).wrapping_add(off * inner),
                dbc: (dbc as *const f32).wrapping_add(off * dbc_ld),
                xz: (xz as *const f32).wrapping_add(off * 2 * inner),
                a_t: layer.a_t.as_ptr(),
                d: layer.d.as_ptr(),
                h: h.ptr().wrapping_add(seq * inner * MAMBA_STATE),
                y: y.ptr().wrapping_add(off * inner),
            });
        });
        layer.out_proj.apply(pool, b.y, inner, rows, out, w, false);
    }

    /// `x += norm(output_proj(...))` of a (bi)directional Mamba over `nseq` sequences of `len` rows of `x`.
    #[allow(clippy::too_many_arguments)]
    fn bimamba(
        &self,
        pool: &Pool,
        bm: &BiMamba,
        x: &mut [f32],
        nseq: usize,
        len: usize,
        state: Option<(&mut [f32], &mut [f32])>,
        s: &mut Scratch,
    ) {
        let w = self.cfg.width;
        let rows = nseq * len;
        let Scratch { a, big, u, dbc, delta, y, f, g, cat, h, .. } = s;
        let mut bufs = MambaBufs {
            xz: &mut big[..rows * 4 * w],
            u: &mut u[..rows * 2 * w],
            dbc: &mut dbc[..rows * bm.forward.x_proj.n],
            delta: &mut delta[..rows * 2 * w],
            y: &mut y[..rows * 2 * w],
            h: &mut h[..],
        };
        let f = &mut f[..rows * w];
        self.mamba(pool, &bm.forward, x, nseq, len, false, state, &mut bufs, f);
        let a = &mut a[..rows * w];
        if let Some(backward) = &bm.backward {
            let g = &mut g[..rows * w];
            self.mamba(pool, backward, x, nseq, len, true, None, &mut bufs, g);
            let cat = &mut cat[..rows * 2 * w];
            let c = SharedMut::new(cat);
            let (fr, gr, xr) = (&*f, &*g, &*x);
            par_rows(pool, rows, 64, |r0, r1| {
                for r in r0..r1 {
                    let dst = unsafe { c.slice(r * 2 * w, 2 * w) };
                    for i in 0..w {
                        dst[i] = fr[r * w + i] + xr[r * w + i];
                        dst[w + i] = gr[r * w + i] + xr[r * w + i];
                    }
                }
            });
            bm.output_proj.apply(pool, cat, 2 * w, rows, a, w, false);
        } else {
            add_into(pool, f, x);
            bm.output_proj.apply(pool, f, w, rows, a, w, false);
        }
        bm.norm.apply(pool, a, rows, f);
        add_into(pool, x, f);
    }

    fn rope_table(&self, pos0: usize, frames: usize, out: &mut [f32]) {
        let hd = self.head_dim();
        for t in 0..frames {
            self.rope_at(pos0 + t, &mut out[t * hd..(t + 1) * hd]);
        }
    }

    fn apply_rope(&self, pool: &Pool, qkv: &mut [f32], rows: usize, frames: usize, rope: &[f32]) {
        let (w, hd, heads) = (self.cfg.width, self.head_dim(), self.cfg.num_heads);
        let half = hd / 2;
        let q = SharedMut::new(qkv);
        par_rows(pool, rows, 64, |r0, r1| {
            for r in r0..r1 {
                let t = r % frames;
                let table = &rope[t * hd..(t + 1) * hd];
                let row = unsafe { q.slice(r * 3 * w, 2 * w) };
                for part in 0..2 {
                    for head in 0..heads {
                        let v = &mut row[part * w + head * hd..part * w + (head + 1) * hd];
                        for i in 0..half {
                            let (c, s) = (table[2 * i], table[2 * i + 1]);
                            let (x1, x2) = (v[i], v[half + i]);
                            v[i] = x1 * c - x2 * s;
                            v[half + i] = x1 * s + x2 * c;
                        }
                    }
                }
            }
        });
    }

    /// The time transformer over `nb` bands (from band `band0`) of `frames` rows each, in place.
    #[allow(clippy::too_many_arguments)]
    fn time_attention(
        &self,
        pool: &Pool,
        ta: &TimeAttention,
        x: &mut [f32],
        nb: usize,
        frames: usize,
        band0: usize,
        ring: Option<Ring>,
        s: &mut Scratch,
    ) {
        let (w, hd, heads) = (self.cfg.width, self.head_dim(), self.cfg.num_heads);
        let rows = nb * frames;
        let pos0 = ring.as_ref().map_or(0, |r| r.2);
        ta.norm1.apply(pool, x, rows, &mut s.a);
        ta.qkv.apply(pool, &s.a, w, rows, &mut s.qkv, 3 * w, false);
        let mut rope = std::mem::take(&mut s.rope);
        self.rope_table(pos0, frames, &mut rope);
        self.apply_rope(pool, &mut s.qkv, rows, frames, &rope);
        s.rope = rope;
        let qkv = s.qkv.as_ptr() as usize;
        let o = SharedMut::new(&mut s.o);
        let ptl = s.per_thread_len;
        let pt = SharedMut::new(&mut s.per_thread);
        match ring {
            None => pool.run(nb * heads, |task, thread| {
                let (band, head) = (task / heads, task % heads);
                seq_attn(&SeqAttn {
                    qkv: (qkv as *const f32).wrapping_add(band * frames * 3 * w),
                    len: frames,
                    head,
                    hd,
                    width: w,
                    causal: self.cfg.causal,
                    window: self.cfg.attention_window,
                    exclusive: self.cfg.exclusive_time,
                    scratch: pt.ptr().wrapping_add(thread * ptl),
                    out: o.ptr().wrapping_add(band * frames * w),
                });
            }),
            Some((keys, values, pos0, cache_frames, capacity)) => {
                let (k, v) = (SharedMut::new(keys), SharedMut::new(values));
                pool.run(nb * heads, |task, thread| {
                    let (band, head) = (task / heads, task % heads);
                    let at = ((band0 + band) * heads + head) * capacity * hd;
                    ring_attn(&RingAttn {
                        qkv: (qkv as *const f32).wrapping_add(band * frames * 3 * w),
                        frames,
                        pos0,
                        head,
                        hd,
                        width: w,
                        window: self.cfg.attention_window,
                        cache_frames,
                        capacity,
                        exclusive: self.cfg.exclusive_time,
                        keys: k.ptr().wrapping_add(at),
                        values: v.ptr().wrapping_add(at),
                        scratch: pt.ptr().wrapping_add(thread * ptl),
                        out: o.ptr().wrapping_add(band * frames * w),
                    });
                });
            }
        }
        ta.proj.apply(pool, &s.o, w, rows, x, w, true);
        ta.norm2.apply(pool, x, rows, &mut s.a);
        ta.ffn1.apply(pool, &s.a, w, rows, &mut s.big, 4 * w, false);
        par_gelu(pool, &mut s.big[..rows * 4 * w]);
        ta.ffn2.apply(pool, &s.big, 4 * w, rows, x, w, true);
    }

    /// The band transformer over `nf` frames of `bands` rows each, in place.
    fn band_attention(&self, pool: &Pool, ba: &BandAttention, x: &mut [f32], nf: usize, s: &mut Scratch) {
        let (w, hd, heads, bands) = (self.cfg.width, self.head_dim(), self.cfg.num_heads, self.bands());
        let rows = nf * bands;
        ba.norm1.apply(pool, x, rows, &mut s.a);
        ba.in_proj.apply(pool, &s.a, w, rows, &mut s.qkv, 3 * w, false);
        let qkv = s.qkv.as_ptr() as usize;
        let o = SharedMut::new(&mut s.o);
        let ptl = s.per_thread_len;
        let pt = SharedMut::new(&mut s.per_thread);
        pool.run(nf * heads, |task, thread| {
            let (frame, head) = (task / heads, task % heads);
            seq_attn(&SeqAttn {
                qkv: (qkv as *const f32).wrapping_add(frame * bands * 3 * w),
                len: bands,
                head,
                hd,
                width: w,
                causal: false,
                window: 0,
                exclusive: self.cfg.exclusive_band,
                scratch: pt.ptr().wrapping_add(thread * ptl),
                out: o.ptr().wrapping_add(frame * bands * w),
            });
        });
        ba.out_proj.apply(pool, &s.o, w, rows, x, w, true);
        ba.norm2.apply(pool, x, rows, &mut s.a);
        ba.lin1.apply(pool, &s.a, w, rows, &mut s.big, 4 * w, false);
        par_gelu(pool, &mut s.big[..rows * 4 * w]);
        ba.lin2.apply(pool, &s.big, 4 * w, rows, x, w, true);
    }

    fn run_blocks(&self, pool: &Pool, s: &mut Scratch, frames: usize, mut stream: Option<&mut StreamState>) {
        let (w, bands) = (self.cfg.width, self.bands());
        let tokens = bands * frames;
        let mut x = std::mem::take(&mut s.x);
        let mut xt = std::mem::take(&mut s.xt);
        let tg = time_group(bands, frames);
        let fg = band_group(bands, frames);
        if stream.is_some() {
            assert_eq!(tg, bands, "um bloco do fluxo cabe num grupo só");
        }
        let (mut ai, mut mi) = (0, 0);
        for block in &self.blocks {
            match block {
                Block::Attention { time, band } => {
                    for g0 in (0..bands).step_by(tg) {
                        let nb = tg.min(bands - g0);
                        let xg = &mut x[g0 * frames * w..(g0 + nb) * frames * w];
                        let ring = stream.as_deref_mut().map(|st| {
                            let (pos, cache, cap) = (st.pos, st.cache_frames, st.capacity);
                            (&mut st.keys[ai][..], &mut st.values[ai][..], pos, cache, cap)
                        });
                        self.time_attention(pool, time, xg, nb, frames, g0, ring, s);
                    }
                    transpose(pool, &x[..tokens * w], &mut xt[..tokens * w], bands, frames, w);
                    for f0 in (0..frames).step_by(fg) {
                        let nf = fg.min(frames - f0);
                        self.band_attention(pool, band, &mut xt[f0 * bands * w..(f0 + nf) * bands * w], nf, s);
                    }
                    transpose(pool, &xt[..tokens * w], &mut x[..tokens * w], frames, bands, w);
                    ai += 1;
                }
                Block::Mamba { time, freq } => {
                    for g0 in (0..bands).step_by(tg) {
                        let nb = tg.min(bands - g0);
                        let xg = &mut x[g0 * frames * w..(g0 + nb) * frames * w];
                        let st = stream.as_deref_mut().map(|st| (&mut st.tails[mi][..], &mut st.states[mi][..]));
                        self.bimamba(pool, time, xg, nb, frames, st, s);
                    }
                    transpose(pool, &x[..tokens * w], &mut xt[..tokens * w], bands, frames, w);
                    for f0 in (0..frames).step_by(fg) {
                        let nf = fg.min(frames - f0);
                        self.bimamba(pool, freq, &mut xt[f0 * bands * w..(f0 + nf) * bands * w], nf, bands, None, s);
                    }
                    transpose(pool, &xt[..tokens * w], &mut x[..tokens * w], frames, bands, w);
                    mi += 1;
                }
            }
        }
        s.x = x;
        s.xt = xt;
    }

    /// Band tokens of spectra `[frames][bins]` into `s.x`.
    fn encode(&self, pool: &Pool, s: &mut Scratch, spec: &[Complex32], frames: usize) {
        let (w, bins) = (self.cfg.width, self.cfg.bins());
        let ptl = s.per_thread_len;
        let (x, pt) = (SharedMut::new(&mut s.x), SharedMut::new(&mut s.per_thread));
        pool.run(self.bands(), |band, thread| {
            let enc = &self.encoders[band];
            let k = 2 * (enc.end - enc.start);
            let buf = unsafe { pt.slice(thread * ptl, 2 * frames * k) };
            let (inp, normed) = buf.split_at_mut(frames * k);
            for t in 0..frames {
                for (i, c) in spec[t * bins + enc.start..t * bins + enc.end].iter().enumerate() {
                    inp[t * k + 2 * i] = c.re;
                    inp[t * k + 2 * i + 1] = c.im;
                }
            }
            kernels::layer_norm(inp, k, &enc.norm.g, &enc.norm.b, normed);
            let out = unsafe { x.slice(band * frames * w, frames * w) };
            enc.lin.apply_st(normed, k, frames, out, w, false);
            let add = &self.band_add[band * w..(band + 1) * w];
            for row in out.chunks_exact_mut(w) {
                for (v, a) in row.iter_mut().zip(add) {
                    *v += a;
                }
            }
        });
    }

    /// The per-band heads over `s.x` into `raw` `[frames][bins][4]`.
    fn decode(&self, pool: &Pool, s: &mut Scratch, frames: usize, raw: &mut [f32]) {
        let (w, bins) = (self.cfg.width, self.cfg.bins());
        let ptl = s.per_thread_len;
        let x = &s.x;
        let (pt, r) = (SharedMut::new(&mut s.per_thread), SharedMut::new(raw));
        pool.run(self.bands(), |band, thread| {
            let head = &self.heads[band];
            let start = self.edges[band].0;
            let buf = unsafe { pt.slice(thread * ptl, 2 * frames * w) };
            let (t1, t2) = buf.split_at_mut(frames * w);
            for (dst, src) in t1.chunks_exact_mut(w).zip(x[band * frames * w..(band + 1) * frames * w].chunks_exact(w)) {
                for i in 0..w {
                    dst[i] = src[i] * self.dec_scale[i] + self.dec_shift[i];
                }
            }
            kernels::layer_norm(t1, w, &head.norm.g, &head.norm.b, t2);
            head.lin1.apply_st(t2, w, frames, t1, w, false);
            kernels::gelu(t1);
            // the band's bins of every frame; other bands write the columns in between
            let out = unsafe { r.slice(start * 4, (frames - 1) * bins * 4 + head.lin2.n) };
            head.lin2.apply_st(t1, w, frames, out, bins * 4, false);
        });
    }

    /// The complex change of frame `spec` that the decoder output `raw` `[bins][4]` encodes.
    pub fn correct(&self, raw: &[f32], spec: &[Complex32], out: &mut [Complex32]) {
        let bound = self.cfg.correction_bound;
        for ((r, s), o) in raw.chunks_exact(4).zip(spec).zip(out.iter_mut()) {
            let m = Complex32::new(2.0 * r[0].tanh(), 2.0 * r[1].tanh());
            let (c0, c1) = (r[2].tanh(), r[3].tanh());
            let norm = (c0 * c0 + c1 * c1).sqrt().max(1.0);
            let mag = bound * s.norm();
            *o = s * m + Complex32::new(c0 / norm * mag, c1 / norm * mag);
        }
    }

    /// Encoder, blocks and heads over spectra `[frames][bins]`: the decoder's output `[frames][bins][4]`.
    pub fn run(
        &self,
        pool: &Pool,
        s: &mut Scratch,
        spec: &[Complex32],
        frames: usize,
        stream: Option<&mut StreamState>,
        raw: &mut [f32],
    ) {
        assert!(frames * self.bands() * self.cfg.width <= s.x.len(), "scratch pequeno demais para {frames} quadros");
        self.encode(pool, s, spec, frames);
        self.run_blocks(pool, s, frames, stream);
        self.decode(pool, s, frames, raw);
    }

    /// Upstream's whole-clip `VoiceIsolatorNetwork.forward`: STFT (centred, reflect-padded), network, look-ahead read
    /// (causal models), correction, iSTFT, added to the input.
    pub fn forward_clip(&self, pool: &Pool, s: &mut Scratch, fft: &mut Fft, audio: &[f32]) -> Vec<f32> {
        let (n, hop, bins) = (self.cfg.n_fft, self.cfg.hop, self.cfg.bins());
        let len = audio.len();
        let pad = n / 2;
        let mut padded = vec![0f32; len + n];
        padded[pad..pad + len].copy_from_slice(audio);
        if len > pad {
            for i in 0..pad {
                padded[pad - 1 - i] = audio[i + 1];
                padded[pad + len + i] = audio[len - 2 - i];
            }
        }
        let frames = 1 + len / hop;
        let mut spec = vec![Complex32::default(); frames * bins];
        for t in 0..frames {
            fft.forward(&padded[t * hop..t * hop + n], &self.window, &mut spec[t * bins..(t + 1) * bins]);
        }
        let mut raw = vec![0f32; frames * bins * 4];
        self.run(pool, s, &spec, frames, None, &mut raw);
        let la = if self.cfg.causal { self.lookahead } else { 0 };
        let total = n + hop * (frames - 1);
        let (mut y, mut env) = (vec![0f32; total], vec![0f32; total]);
        let mut corrected = vec![Complex32::default(); bins];
        let mut frame = vec![0f32; n];
        for t in 0..frames {
            let src = (t + la).min(frames - 1);
            self.correct(&raw[src * bins * 4..(src + 1) * bins * 4], &spec[t * bins..(t + 1) * bins], &mut corrected);
            fft.inverse(&corrected, &self.window, &mut frame);
            for i in 0..n {
                y[t * hop + i] += frame[i];
                env[t * hop + i] += self.window[i] * self.window[i];
            }
        }
        (0..len)
            .map(|i| {
                let e = env[i + pad];
                audio[i] + if e > 1e-11 { y[i + pad] / e } else { 0.0 }
            })
            .collect()
    }
}

/// Real FFTs of the analysis window.
pub struct Fft {
    pub n: usize,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    buf: Vec<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(n);
        let c2r = planner.plan_fft_inverse(n);
        let len = r2c.get_scratch_len().max(c2r.get_scratch_len());
        Fft {
            n,
            r2c,
            c2r,
            buf: vec![0.0; n],
            spec: vec![Complex32::default(); n / 2 + 1],
            scratch: vec![Complex32::default(); len],
        }
    }

    /// The spectrum of `frame · window`.
    pub fn forward(&mut self, frame: &[f32], window: &[f32], out: &mut [Complex32]) {
        for ((b, f), w) in self.buf.iter_mut().zip(frame).zip(window) {
            *b = f * w;
        }
        self.r2c.process_with_scratch(&mut self.buf, out, &mut self.scratch).expect("FFT");
    }

    /// `irfft(spec) · window` with the 1/n of numpy and torch; DC and Nyquist are read as real, as by every C2R.
    pub fn inverse(&mut self, spec: &[Complex32], window: &[f32], out: &mut [f32]) {
        self.spec.copy_from_slice(spec);
        let last = self.spec.len() - 1;
        self.spec[0].im = 0.0;
        self.spec[last].im = 0.0;
        self.c2r.process_with_scratch(&mut self.spec, out, &mut self.scratch).expect("IFFT");
        let scale = 1.0 / self.n as f32;
        for (o, w) in out.iter_mut().zip(window) {
            *o *= scale * w;
        }
    }
}
