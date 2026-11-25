//! **Isolar voz**: the Inworld Voice Isolator v1 (code and weights Apache-2.0, <https://huggingface.co/inworld/voice-isolator>)
//! in pure Rust, for Kalango Audio.
//!
//! - [`Offline`]: recorded audio through `denoiser.safetensors` (two-sided, 64M parameters), in 8 s windows
//!   overlapping by 2 s with a linear crossfade, as upstream's `enhance`.
//! - [`Realtime`]: a live stream through `denoiser_realtime.safetensors` (causal, 58M parameters), 20 ms of
//!   look-ahead, 63 ms of algorithmic latency, state carried across pushes.
//!
//! Both take and return mono float samples at 48 kHz; resampling is the caller's. The checkpoints are memory-mapped
//! and read in place. The math follows upstream `model.py` and `streaming.py` (v1.0.0) and is checked against the
//! PyTorch reference by the tests in `tests/reference.rs`.

mod kernels;
pub mod metrics;
mod network;
mod pool;
mod realtime;
mod weights;

use std::path::Path;
use std::sync::Arc;

pub use realtime::{Realtime, RealtimeOptions};

/// Why a checkpoint could not be used.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Format(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "não foi possível ler o modelo: {e}"),
            Error::Format(e) => write!(f, "modelo inválido: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// File names of the released checkpoints on the Hugging Face repository `inworld/voice-isolator`.
pub const OFFLINE_FILE: &str = "denoiser.safetensors";
pub const REALTIME_FILE: &str = "denoiser_realtime.safetensors";
/// SHA-256 of the released v1 checkpoints (upstream `registry.py`, revision 9b0285b3).
pub const OFFLINE_SHA256: &str = "647335a6aeb4dc706b895532c209d51e0e84a4849d17b339f00d4392a050ae53";
pub const REALTIME_SHA256: &str = "76598f05523c0669e76562a8b2f31c9d395fca316e80c907ee11775e7497ae0f";

/// The sample rate every model of v1 runs at.
pub const SAMPLE_RATE: u32 = 48_000;

/// Recorded audio through a checkpoint, whole clip at a time.
pub struct Offline {
    net: Arc<network::Network>,
    pool: pool::Pool,
    scratch: Option<(usize, network::Scratch)>,
    fft: network::Fft,
    chunk: usize,
    overlap: usize,
}

impl Offline {
    /// Opens a checkpoint on `threads` threads. The offline checkpoint is the one made for this; the realtime one
    /// runs here as its causal whole-clip forward (look-ahead 2), which is what its stream equals.
    pub fn open(path: &Path, threads: usize) -> Result<Offline, Error> {
        let ck = Arc::new(weights::Checkpoint::open(path)?);
        let net = Arc::new(network::Network::load(ck, None)?);
        let cfg = &net.cfg;
        let rate = cfg.sample_rate as f64;
        let chunk = (cfg.chunk_seconds * rate) as usize;
        let overlap = ((cfg.overlap_seconds * rate) as usize).min(chunk / 2);
        Ok(Offline {
            pool: pool::Pool::new(threads),
            scratch: None,
            fft: network::Fft::new(cfg.n_fft),
            chunk,
            overlap,
            net,
        })
    }

    /// Parameters of the opened checkpoint.
    pub fn is_causal(&self) -> bool {
        self.net.cfg.causal
    }

    /// Uses windows of `chunk_seconds` overlapping by `overlap_seconds` instead of the checkpoint's own.
    pub fn set_window(&mut self, chunk_seconds: f64, overlap_seconds: f64) {
        let rate = self.net.cfg.sample_rate as f64;
        self.chunk = ((chunk_seconds * rate) as usize).max(self.net.cfg.hop);
        self.overlap = ((overlap_seconds.max(0.0) * rate) as usize).min(self.chunk / 2);
    }

    fn pass(&mut self, audio: &[f32]) -> Vec<f32> {
        let frames = 1 + audio.len() / self.net.cfg.hop;
        if self.scratch.as_ref().is_none_or(|(f, _)| *f < frames) {
            self.scratch = None;
            self.scratch = Some((frames, network::Scratch::new(&self.net, frames, self.pool.threads(), 0)));
        }
        let scratch = &mut self.scratch.as_mut().unwrap().1;
        self.net.forward_clip(&self.pool, scratch, &mut self.fft, audio)
    }

    /// Isolates the voice of mono 48 kHz `audio` of any length.
    pub fn process(&mut self, audio: &[f32]) -> Vec<f32> {
        let n = audio.len();
        if n == 0 {
            return Vec::new();
        }
        let (chunk, overlap) = (self.chunk, self.overlap);
        if n <= chunk {
            return self.pass(audio);
        }
        let mut out = vec![0f32; n];
        let mut weight = vec![0f32; n];
        let mut starts: Vec<usize> = (0..n - chunk).step_by(chunk - overlap).collect();
        starts.push(n - chunk);
        let ramp = |i: usize| if overlap > 1 { i as f32 / (overlap - 1) as f32 } else { 1.0 };
        for (index, &start) in starts.iter().enumerate() {
            let end = start + chunk;
            let mut w = vec![1f32; chunk];
            if index > 0 {
                let fade_in = starts[index - 1] + chunk - overlap - start;
                w[..fade_in].fill(0.0);
                for i in 0..overlap {
                    w[fade_in + i] = ramp(i);
                }
            }
            if overlap > 0 && end < n {
                for i in 0..overlap {
                    w[chunk - overlap + i] = 1.0 - ramp(i);
                }
            }
            let y = self.pass(&audio[start..end]);
            for i in 0..chunk {
                out[start + i] += y[i] * w[i];
                weight[start + i] += w[i];
            }
        }
        for (o, w) in out.iter_mut().zip(&weight) {
            *o /= w.max(1e-6);
        }
        out
    }
}
