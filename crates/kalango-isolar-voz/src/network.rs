//! The band-split dual-path network of the Inworld Voice Isolator, ported from upstream `model.py`/`streaming.py`.
//!
//! Tokens live in one buffer laid out `[band][frame][width]` for the time-axis layers (attention along time, the time
//! Mamba) and are transposed to `[frame][band][width]` for the band-axis layers. Long clips are processed in groups
//! of whole sequences (a few bands, or a few frames) so the scratch stays small; a stream's block of new frames is a
//! single group. The same block code serves a whole clip and a stream: the stream passes its carried state (the time
//! attention's past keys and values in a ring, the time Mamba's convolution tail and recurrence).

use std::sync::Arc;

use crate::kernels::{self, dispatch, exp, matmul_nt};
use crate::pool::{Pool, SharedMut};
use crate::weights::{BlockKind, Checkpoint, Config, Tensor, MAMBA_CONV, MAMBA_STATE};
use crate::Error;

mod forward;
pub use forward::Fft;

/// Rows per group of sequences in a long clip.
const GROUP_ROWS: usize = 2048;

pub(crate) struct Linear {
    w: Tensor,
    b: Option<Tensor>,
    n: usize,
    k: usize,
}

impl Linear {
    fn load(ck: &Checkpoint, prefix: &str, n: usize, k: usize, bias: bool) -> Result<Linear, Error> {
        Ok(Linear {
            w: ck.get(&format!("{prefix}.weight"), &[n, k])?,
            b: if bias { Some(ck.get(&format!("{prefix}.bias"), &[n])?) } else { None },
            n,
            k,
        })
    }

    fn from_parts(w: Tensor, b: Option<Tensor>, n: usize, k: usize) -> Linear {
        Linear { w, b, n, k }
    }

    /// `out[r] (+)= x[r] · wᵀ + b` for `m` rows, `x` at stride `ldx`, `out` at stride `ldo`, on one thread.
    fn apply_st(&self, x: &[f32], ldx: usize, m: usize, out: &mut [f32], ldo: usize, add: bool) {
        assert!(m == 0 || (x.len() >= (m - 1) * ldx + self.k && out.len() >= (m - 1) * ldo + self.n));
        unsafe {
            matmul_nt(x.as_ptr(), ldx, m, self.k, self.w.as_ptr(), self.k, 0, self.n, self.b.as_deref(), out.as_mut_ptr(), ldo, add)
        }
    }

    /// As [`apply_st`], split over the pool: by rows for many rows, by outputs for a stream's few.
    fn apply(&self, pool: &Pool, x: &[f32], ldx: usize, m: usize, out: &mut [f32], ldo: usize, add: bool) {
        let threads = pool.threads();
        if threads == 1 || m * self.n * self.k < (1 << 19) {
            return self.apply_st(x, ldx, m, out, ldo, add);
        }
        assert!(x.len() >= (m - 1) * ldx + self.k && out.len() >= (m - 1) * ldo + self.n);
        let (xp, op) = (SharedMut(x.as_ptr() as *mut f32), SharedMut::new(out));
        let (w, b, k, n) = (self.w.as_ptr() as usize, self.b.as_deref(), self.k, self.n);
        if m >= 256 {
            let rows = 64;
            pool.run(m.div_ceil(rows), |task, _| {
                let r0 = task * rows;
                let r1 = (r0 + rows).min(m);
                unsafe { matmul_nt(xp.ptr().add(r0 * ldx), ldx, r1 - r0, k, w as *const f32, k, 0, n, b, op.ptr().add(r0 * ldo), ldo, add) }
            });
        } else {
            let chunk = (n.div_ceil(threads * 2).div_ceil(3) * 3).max(12);
            pool.run(n.div_ceil(chunk), |task, _| {
                let n0 = task * chunk;
                let n1 = (n0 + chunk).min(n);
                unsafe { matmul_nt(xp.ptr(), ldx, m, k, w as *const f32, k, n0, n1, b, op.ptr(), ldo, add) }
            });
        }
    }
}

struct Norm {
    g: Tensor,
    b: Tensor,
}

impl Norm {
    fn load(ck: &Checkpoint, prefix: &str, n: usize) -> Result<Norm, Error> {
        Ok(Norm { g: ck.get(&format!("{prefix}.weight"), &[n])?, b: ck.get(&format!("{prefix}.bias"), &[n])? })
    }

    fn apply(&self, pool: &Pool, x: &[f32], rows: usize, out: &mut [f32]) {
        let width = self.g.len();
        par_rows(pool, rows, 64, |r0, r1| unsafe {
            let src = std::slice::from_raw_parts(x.as_ptr().add(r0 * width), (r1 - r0) * width);
            let dst = std::slice::from_raw_parts_mut(out.as_ptr().add(r0 * width) as *mut f32, (r1 - r0) * width);
            kernels::layer_norm(src, width, &self.g, &self.b, dst);
        });
    }
}

/// Runs `f(r0, r1)` over row ranges of `chunk` rows on the pool. `f` must touch only its own rows.
fn par_rows<F: Fn(usize, usize) + Sync>(pool: &Pool, rows: usize, chunk: usize, f: F) {
    if rows <= chunk || pool.threads() == 1 {
        return f(0, rows);
    }
    pool.run(rows.div_ceil(chunk), |task, _| {
        let r0 = task * chunk;
        f(r0, (r0 + chunk).min(rows));
    });
}

struct TimeAttention {
    norm1: Norm,
    qkv: Linear,
    proj: Linear,
    norm2: Norm,
    ffn1: Linear,
    ffn2: Linear,
}

struct BandAttention {
    norm1: Norm,
    in_proj: Linear,
    out_proj: Linear,
    norm2: Norm,
    lin1: Linear,
    lin2: Linear,
}

struct MambaLayer {
    in_proj: Linear,
    conv_w: Tensor,
    conv_b: Tensor,
    x_proj: Linear,
    dt_proj: Linear,
    /// `-exp(A_log)` transposed to `[state][channel]`.
    a_t: Vec<f32>,
    d: Tensor,
    out_proj: Linear,
}

struct BiMamba {
    forward: MambaLayer,
    backward: Option<MambaLayer>,
    output_proj: Linear,
    norm: Norm,
}

enum Block {
    Attention { time: TimeAttention, band: BandAttention },
    Mamba { time: BiMamba, freq: BiMamba },
}

struct Encoder {
    norm: Norm,
    lin: Linear,
    start: usize,
    end: usize,
}

struct Head {
    norm: Norm,
    lin1: Linear,
    lin2: Linear,
}

pub struct Network {
    _checkpoint: Arc<Checkpoint>,
    pub cfg: Config,
    pub edges: Vec<(usize, usize)>,
    encoders: Vec<Encoder>,
    /// band embedding plus the look-ahead embedding of the streaming position, `[band][width]`
    band_add: Vec<f32>,
    blocks: Vec<Block>,
    heads: Vec<Head>,
    dec_scale: Vec<f32>,
    dec_shift: Vec<f32>,
    pub window: Vec<f32>,
    /// look-ahead in force (frames): the checkpoint's default for a causal model, zero for a two-sided one
    pub lookahead: usize,
    inv_freq: Vec<f32>,
}

fn load_mamba(ck: &Checkpoint, p: &str, width: usize, inner: usize, dt_rank: usize) -> Result<MambaLayer, Error> {
    let a_log = ck.get(&format!("{p}.A_log"), &[inner, MAMBA_STATE])?;
    let mut a_t = vec![0f32; inner * MAMBA_STATE];
    for c in 0..inner {
        for s in 0..MAMBA_STATE {
            a_t[s * inner + c] = -a_log[c * MAMBA_STATE + s].exp();
        }
    }
    Ok(MambaLayer {
        in_proj: Linear::load(ck, &format!("{p}.in_proj"), 2 * inner, width, false)?,
        conv_w: ck.get(&format!("{p}.conv1d.weight"), &[inner, 1, MAMBA_CONV])?,
        conv_b: ck.get(&format!("{p}.conv1d.bias"), &[inner])?,
        x_proj: Linear::load(ck, &format!("{p}.x_proj"), dt_rank + 2 * MAMBA_STATE, inner, false)?,
        dt_proj: Linear::load(ck, &format!("{p}.dt_proj"), inner, dt_rank, true)?,
        a_t,
        d: ck.get(&format!("{p}.D"), &[inner])?,
        out_proj: Linear::load(ck, &format!("{p}.out_proj"), width, inner, false)?,
    })
}

fn load_bimamba(ck: &Checkpoint, p: &str, width: usize, causal: bool) -> Result<BiMamba, Error> {
    let inner = 2 * width;
    let dt_rank = width.div_ceil(16);
    Ok(BiMamba {
        forward: load_mamba(ck, &format!("{p}.forward_blocks"), width, inner, dt_rank)?,
        backward: if causal { None } else { Some(load_mamba(ck, &format!("{p}.backward_blocks"), width, inner, dt_rank)?) },
        output_proj: Linear::load(ck, &format!("{p}.output_proj"), width, if causal { width } else { 2 * width }, true)?,
        norm: Norm::load(ck, &format!("{p}.norm"), width)?,
    })
}

impl Network {
    pub fn load(ck: Arc<Checkpoint>, lookahead: Option<usize>) -> Result<Network, Error> {
        let cfg = ck.config.clone();
        let w = cfg.width;
        let edges = cfg.band_edges();
        let bands = edges.len();
        let mut encoders = Vec::with_capacity(bands);
        for (i, &(start, end)) in edges.iter().enumerate() {
            let k = 2 * (end - start);
            encoders.push(Encoder {
                norm: Norm::load(&ck, &format!("encoders.{i}.0"), k)?,
                lin: Linear::load(&ck, &format!("encoders.{i}.1"), w, k, true)?,
                start,
                end,
            });
        }
        let mut band_add = ck.get("band_embedding", &[bands, w])?.to_vec();
        let lookahead = if cfg.causal {
            let frames = lookahead.unwrap_or(cfg.lookahead_frames);
            if !cfg.lookahead_choices.is_empty() {
                let index = cfg.lookahead_choices.iter().position(|&c| c == frames).ok_or_else(|| {
                    Error::Format(format!("look-ahead de {frames} quadros não é um dos treinados: {:?}", cfg.lookahead_choices))
                })?;
                let table = ck.get("lookahead_embedding.weight", &[cfg.lookahead_choices.len(), w])?;
                for band in band_add.chunks_exact_mut(w) {
                    for (v, e) in band.iter_mut().zip(&table[index * w..(index + 1) * w]) {
                        *v += e;
                    }
                }
            } else if frames != cfg.lookahead_frames {
                return Err(Error::Format("este checkpoint não escolhe look-ahead".into()));
            }
            frames
        } else {
            if lookahead.is_some_and(|f| f != 0) {
                return Err(Error::Format("o modelo offline não tem look-ahead".into()));
            }
            0
        };
        let mut blocks = Vec::with_capacity(cfg.blocks.len());
        for (i, kind) in cfg.blocks.iter().enumerate() {
            let p = format!("blocks.{i}");
            blocks.push(match kind {
                BlockKind::Transformer => {
                    let t = format!("{p}.time_transformer");
                    let b = format!("{p}.band_transformer");
                    Block::Attention {
                        time: TimeAttention {
                            norm1: Norm::load(&ck, &format!("{t}.norm1"), w)?,
                            qkv: Linear::load(&ck, &format!("{t}.attn.qkv"), 3 * w, w, true)?,
                            proj: Linear::load(&ck, &format!("{t}.attn.proj"), w, w, true)?,
                            norm2: Norm::load(&ck, &format!("{t}.norm2"), w)?,
                            ffn1: Linear::load(&ck, &format!("{t}.ffn.0"), 4 * w, w, true)?,
                            ffn2: Linear::load(&ck, &format!("{t}.ffn.3"), w, 4 * w, true)?,
                        },
                        band: BandAttention {
                            norm1: Norm::load(&ck, &format!("{b}.norm1"), w)?,
                            in_proj: Linear::from_parts(
                                ck.get(&format!("{b}.self_attn.in_proj_weight"), &[3 * w, w])?,
                                Some(ck.get(&format!("{b}.self_attn.in_proj_bias"), &[3 * w])?),
                                3 * w,
                                w,
                            ),
                            out_proj: Linear::load(&ck, &format!("{b}.self_attn.out_proj"), w, w, true)?,
                            norm2: Norm::load(&ck, &format!("{b}.norm2"), w)?,
                            lin1: Linear::load(&ck, &format!("{b}.linear1"), 4 * w, w, true)?,
                            lin2: Linear::load(&ck, &format!("{b}.linear2"), w, 4 * w, true)?,
                        },
                    }
                }
                BlockKind::Mamba => Block::Mamba {
                    time: load_bimamba(&ck, &format!("{p}.time_mamba"), w, cfg.causal)?,
                    freq: load_bimamba(&ck, &format!("{p}.freq_mamba"), w, false)?,
                },
            });
        }
        let mut heads = Vec::with_capacity(bands);
        for (i, &(start, end)) in edges.iter().enumerate() {
            heads.push(Head {
                norm: Norm::load(&ck, &format!("denoise_decoder.heads.{i}.0"), w)?,
                lin1: Linear::load(&ck, &format!("denoise_decoder.heads.{i}.1"), w, w, true)?,
                lin2: Linear::load(&ck, &format!("denoise_decoder.heads.{i}.3"), 4 * (end - start), w, true)?,
            });
        }
        // the decoder's conditioning on its fixed controls [1, 0]
        let cw = ck.get("denoise_decoder.condition.weight", &[2 * w, 2])?;
        let cb = ck.get("denoise_decoder.condition.bias", &[2 * w])?;
        let cond: Vec<f32> = (0..2 * w).map(|i| cw[2 * i] + cb[i]).collect();
        let n = cfg.n_fft;
        let window = (0..n).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32).collect();
        let half = w / cfg.num_heads / 2;
        let inv_freq = (0..half).map(|i| 10000f32.powf(-(i as f32) / half as f32)).collect();
        Ok(Network {
            _checkpoint: ck,
            edges,
            encoders,
            band_add,
            blocks,
            heads,
            dec_scale: cond[..w].iter().map(|s| 1.0 + s).collect(),
            dec_shift: cond[w..].to_vec(),
            window,
            lookahead,
            inv_freq,
            cfg,
        })
    }

    pub fn bands(&self) -> usize {
        self.edges.len()
    }

    fn head_dim(&self) -> usize {
        self.cfg.width / self.cfg.num_heads
    }

    /// Rotary cosines and sines `[hd/2]` (interleaved cos, sin) of `pos`.
    fn rope_at(&self, pos: usize, out: &mut [f32]) {
        for (i, &inv) in self.inv_freq.iter().enumerate() {
            let angle = pos as f64 * inv as f64;
            out[2 * i] = angle.cos() as f32;
            out[2 * i + 1] = angle.sin() as f32;
        }
    }
}

/// Reusable buffers for one network run of up to `rows` tokens per group.
pub struct Scratch {
    rows: usize,
    a: Vec<f32>,
    qkv: Vec<f32>,
    o: Vec<f32>,
    big: Vec<f32>,
    u: Vec<f32>,
    dbc: Vec<f32>,
    delta: Vec<f32>,
    y: Vec<f32>,
    f: Vec<f32>,
    g: Vec<f32>,
    cat: Vec<f32>,
    /// the recurrence of sequences that carry no state (whole clips, the band axis)
    h: Vec<f32>,
    rope: Vec<f32>,
    /// per-thread scratch for attention, encoders and heads
    per_thread: Vec<f32>,
    per_thread_len: usize,
    pub x: Vec<f32>,
    pub xt: Vec<f32>,
}

impl Scratch {
    /// Buffers for clips of up to `frames` frames (a stream: its block size) on a pool of `threads`.
    pub fn new(net: &Network, frames: usize, threads: usize, ring: usize) -> Scratch {
        let w = net.cfg.width;
        let bands = net.bands();
        let tokens = bands * frames;
        // a group holds whole sequences: a few bands of `frames` rows, or a few frames of `bands` rows
        let rows = GROUP_ROWS.max(frames).max(bands).min(tokens.max(1));
        let hd = w / net.cfg.num_heads;
        let seq = frames.max(bands);
        let attention = seq * seq + 2 * hd * seq + ring + hd;
        let maxw = net.edges.iter().map(|(s, e)| e - s).max().unwrap_or(1);
        let per_thread_len = attention.max(frames * 2 * maxw + 2 * frames * w).max(frames * (2 * w + 4 * maxw));
        Scratch {
            rows,
            a: vec![0.0; rows * w],
            qkv: vec![0.0; rows * 3 * w],
            o: vec![0.0; rows * w],
            big: vec![0.0; rows * 4 * w],
            u: vec![0.0; rows * 2 * w],
            dbc: vec![0.0; rows * (w.div_ceil(16) + 2 * MAMBA_STATE)],
            delta: vec![0.0; rows * 2 * w],
            y: vec![0.0; rows * 2 * w],
            f: vec![0.0; rows * w],
            g: vec![0.0; rows * w],
            cat: vec![0.0; rows * 2 * w],
            h: vec![0.0; bands.max(rows.div_ceil(bands)) * 2 * w * MAMBA_STATE],
            rope: vec![0.0; frames * hd],
            per_thread: vec![0.0; per_thread_len * threads.max(1)],
            per_thread_len,
            x: vec![0.0; tokens * w],
            xt: vec![0.0; tokens * w],
        }
    }
}

/// The carried state of a stream.
pub struct StreamState {
    /// per attention block: keys and values `[band][head][slot][hd]`, a ring of `capacity` slots
    keys: Vec<Vec<f32>>,
    values: Vec<Vec<f32>>,
    /// per Mamba block: the convolution tail `[band][conv - 1][inner]` and the recurrence `[band][inner/8][state][8]`
    tails: Vec<Vec<f32>>,
    states: Vec<Vec<f32>>,
    capacity: usize,
    cache_frames: usize,
    /// frames processed so far
    pub pos: usize,
}

impl StreamState {
    pub fn new(net: &Network, cache_frames: usize, max_block: usize) -> StreamState {
        let w = net.cfg.width;
        let bands = net.bands();
        let capacity = cache_frames + max_block;
        let inner = 2 * w;
        let mut s = StreamState {
            keys: vec![],
            values: vec![],
            tails: vec![],
            states: vec![],
            capacity,
            cache_frames,
            pos: 0,
        };
        for block in &net.blocks {
            match block {
                Block::Attention { .. } => {
                    s.keys.push(vec![0.0; bands * capacity * w]);
                    s.values.push(vec![0.0; bands * capacity * w]);
                }
                Block::Mamba { .. } => {
                    s.tails.push(vec![0.0; bands * (MAMBA_CONV - 1) * inner]);
                    s.states.push(vec![0.0; bands * inner * MAMBA_STATE]);
                }
            }
        }
        s
    }

    pub fn bytes(&self) -> usize {
        4 * (self.keys.iter().chain(&self.values).chain(&self.tails).chain(&self.states).map(Vec::len).sum::<usize>())
    }
}

enum Mode<'a> {
    Clip,
    Stream(&'a mut StreamState),
}

struct ScanArgs {
    rows: usize,
    reverse: bool,
    c0: usize,
    c1: usize,
    inner: usize,
    dbc_ld: usize,
    dt_rank: usize,
    u: *const f32,
    delta: *const f32,
    dbc: *const f32,
    xz: *const f32,
    a_t: *const f32,
    d: *const f32,
    h: *mut f32,
    y: *mut f32,
}

#[inline(always)]
fn scan_body(a: &ScanArgs) {
    unsafe {
        let inner = a.inner;
        let mut c = a.c0;
        while c < a.c1 {
            let h = std::slice::from_raw_parts_mut(a.h.add(c * MAMBA_STATE), 8 * MAMBA_STATE);
            let mut av = [[0f32; 8]; MAMBA_STATE];
            for (n, row) in av.iter_mut().enumerate() {
                row.copy_from_slice(std::slice::from_raw_parts(a.a_t.add(n * inner + c), 8));
            }
            let dv: [f32; 8] = std::slice::from_raw_parts(a.d.add(c), 8).try_into().unwrap();
            let mut hs = [[0f32; 8]; MAMBA_STATE];
            for n in 0..MAMBA_STATE {
                hs[n].copy_from_slice(&h[n * 8..n * 8 + 8]);
            }
            for t in 0..a.rows {
                let r = if a.reverse { a.rows - 1 - t } else { t };
                let bc = std::slice::from_raw_parts(a.dbc.add(r * a.dbc_ld + a.dt_rank), 2 * MAMBA_STATE);
                let dl: [f32; 8] = std::slice::from_raw_parts(a.delta.add(r * inner + c), 8).try_into().unwrap();
                let ul: [f32; 8] = std::slice::from_raw_parts(a.u.add(r * inner + c), 8).try_into().unwrap();
                let zl: [f32; 8] = std::slice::from_raw_parts(a.xz.add(r * 2 * inner + inner + c), 8).try_into().unwrap();
                let mut du = [0f32; 8];
                for l in 0..8 {
                    du[l] = dl[l] * ul[l];
                }
                let mut y = [0f32; 8];
                for n in 0..MAMBA_STATE {
                    let b = bc[n];
                    let cc = bc[MAMBA_STATE + n];
                    for l in 0..8 {
                        hs[n][l] = exp(dl[l] * av[n][l]) * hs[n][l] + du[l] * b;
                        y[l] += hs[n][l] * cc;
                    }
                }
                let out = std::slice::from_raw_parts_mut(a.y.add(r * inner + c), 8);
                for l in 0..8 {
                    let g = zl[l] / (1.0 + exp(-zl[l]));
                    out[l] = (y[l] + dv[l] * ul[l]) * g;
                }
            }
            for n in 0..MAMBA_STATE {
                h[n * 8..n * 8 + 8].copy_from_slice(&hs[n]);
            }
            c += 8;
        }
    }
}

dispatch! {
    fn scan(a: &ScanArgs) => scan_body
}

struct ConvArgs {
    rows: usize,
    reverse: bool,
    c0: usize,
    c1: usize,
    inner: usize,
    xz: *const f32,
    w: *const f32,
    b: *const f32,
    tail: *mut f32,
    out: *mut f32,
}

#[inline(always)]
fn conv_body(a: &ConvArgs) {
    unsafe {
        let inner = a.inner;
        let k = MAMBA_CONV;
        let n = a.c1 - a.c0;
        let wt = |c: usize, j: usize| *a.w.add(c * k + j);
        // the history before this run: the carried tail, or zeros
        let input = |t: isize, c: usize| -> f32 {
            if t >= 0 {
                let t = t as usize;
                let r = if a.reverse { a.rows - 1 - t } else { t };
                *a.xz.add(r * 2 * inner + c)
            } else if !a.tail.is_null() {
                *a.tail.add(((k as isize - 1 + t) as usize) * inner + c)
            } else {
                0.0
            }
        };
        for t in 0..a.rows {
            let r = if a.reverse { a.rows - 1 - t } else { t };
            let out = std::slice::from_raw_parts_mut(a.out.add(r * inner + a.c0), n);
            for (i, o) in out.iter_mut().enumerate() {
                let c = a.c0 + i;
                let mut acc = *a.b.add(c);
                for j in 0..k {
                    acc += wt(c, j) * input(t as isize - (k as isize - 1) + j as isize, c);
                }
                *o = acc / (1.0 + exp(-acc));
            }
        }
        if !a.tail.is_null() {
            for c in a.c0..a.c1 {
                let mut last = [0f32; MAMBA_CONV - 1];
                for (j, v) in last.iter_mut().enumerate() {
                    *v = input(a.rows as isize - (k as isize - 1) + j as isize, c);
                }
                for (j, v) in last.iter().enumerate() {
                    *a.tail.add(j * inner + c) = *v;
                }
            }
        }
    }
}

dispatch! {
    fn conv(a: &ConvArgs) => conv_body
}

#[inline(always)]
fn softplus_body(x: &mut [f32]) {
    for v in x.iter_mut() {
        if *v <= 20.0 {
            *v = (1.0 + exp(*v)).ln();
        }
    }
}

dispatch! {
    fn softplus(x: &mut [f32]) => softplus_body
}

