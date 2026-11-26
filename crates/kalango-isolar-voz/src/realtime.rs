//! Streaming: the isolated voice leaves as the audio arrives (upstream `streaming.Stream`, plain stream).
//!
//! Every block of new frames runs once with the state carried (time attention over a ring of past keys, the time
//! Mamba's recurrence, the STFT's overlap-add), and output samples are released as soon as every frame that touches
//! them has been synthesized. All buffers are allocated up front: `push_frame` allocates nothing.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use realfft::num_complex::Complex32;

use crate::network::{Fft, Network, Scratch, StreamState};
use crate::pool::Pool;
use crate::weights::Checkpoint;
use crate::Error;

/// Settings of a [`Realtime`] stream.
#[derive(Clone, Debug)]
pub struct RealtimeOptions {
    /// Look-ahead in frames of 10 ms: one of the checkpoint's trained positions (0, 2 or 4 for v1).
    pub lookahead_frames: usize,
    /// Threads running the network, the caller's included.
    pub threads: usize,
    /// Past frames the time attention may see. `None`: the checkpoint's attention window (1200 = 12 s for v1), which
    /// makes the stream equal to the whole-clip forward; fewer frames cost less memory and memory bandwidth.
    pub cache_frames: Option<usize>,
    /// Most frames computed together (a throughput knob; it adds no latency).
    pub block_frames: usize,
}

impl Default for RealtimeOptions {
    fn default() -> Self {
        RealtimeOptions { lookahead_frames: 2, threads: 1, cache_frames: None, block_frames: 2 }
    }
}

/// A live stream through the realtime checkpoint.
pub struct Realtime {
    net: Arc<Network>,
    pool: Pool,
    scratch: Scratch,
    state: StreamState,
    fft: Fft,
    block: usize,
    lookahead: usize,
    n: usize,
    hop: usize,
    bins: usize,
    audio: Vec<f32>,
    audio_base: usize,
    frames: usize,
    emitted: usize,
    ring: usize,
    raw_ring: Vec<f32>,
    spec_ring: Vec<Complex32>,
    block_spec: Vec<Complex32>,
    block_raw: Vec<f32>,
    segment: Vec<f32>,
    frame: Vec<f32>,
    corrected: Vec<Complex32>,
    ola: Vec<f32>,
    env: Vec<f32>,
    ola_base: usize,
    released: usize,
    closed: bool,
    fifo: VecDeque<f32>,
    fifo_frame: usize,
    out: Vec<f32>,
}

impl Realtime {
    /// Opens the realtime checkpoint (`denoiser_realtime.safetensors`).
    pub fn open(path: &Path, options: RealtimeOptions) -> Result<Realtime, Error> {
        let ck = Arc::new(Checkpoint::open(path)?);
        if !ck.config.causal {
            return Err(Error::Format("este checkpoint é offline: o tempo real precisa do modelo causal".into()));
        }
        let net = Arc::new(Network::load(ck, Some(options.lookahead_frames))?);
        Ok(Realtime::with_network(net, options))
    }

    pub(crate) fn with_network(net: Arc<Network>, options: RealtimeOptions) -> Realtime {
        let cfg = &net.cfg;
        let (n, hop, bins) = (cfg.n_fft, cfg.hop, cfg.bins());
        let block = options.block_frames.max(1);
        let cache = options.cache_frames.unwrap_or(if cfg.attention_window > 0 {
            cfg.attention_window
        } else {
            ((cfg.chunk_seconds * cfg.sample_rate as f64 / hop as f64).round() as usize).max(1)
        });
        let threads = options.threads.max(1);
        let state = StreamState::new(&net, cache, block);
        let scratch = Scratch::new(&net, block, threads, cache + block);
        let lookahead = net.lookahead;
        let ring = lookahead + block + 2;
        let piece = block * hop;
        Realtime {
            pool: Pool::new(threads),
            scratch,
            state,
            fft: Fft::new(n),
            block,
            lookahead,
            n,
            hop,
            bins,
            audio: Vec::with_capacity(2 * n + (lookahead + 2 * block + 4) * hop + piece),
            audio_base: 0,
            frames: 0,
            emitted: 0,
            ring,
            raw_ring: vec![0.0; ring * bins * 4],
            spec_ring: vec![Complex32::default(); ring * bins],
            block_spec: vec![Complex32::default(); block * bins],
            block_raw: vec![0.0; block * bins * 4],
            segment: vec![0.0; n],
            frame: vec![0.0; n],
            corrected: vec![Complex32::default(); bins],
            ola: vec![0.0; 2 * n + (block + lookahead + 8) * hop],
            env: vec![0.0; 2 * n + (block + lookahead + 8) * hop],
            ola_base: 0,
            released: 0,
            closed: false,
            fifo: VecDeque::new(),
            fifo_frame: 0,
            out: Vec::with_capacity(2 * n + (lookahead + 2 * block + 4) * hop + piece),
            net,
        }
    }

    /// Input samples that must follow an output sample before its release (upstream `latency_samples`).
    pub fn latency_samples(&self) -> usize {
        self.n + self.hop * self.lookahead
    }

    pub fn sample_rate(&self) -> u32 {
        self.net.cfg.sample_rate
    }

    /// Bytes of the carried state (mostly the attention's past keys and values).
    pub fn state_bytes(&self) -> usize {
        self.state.bytes()
    }

    /// Feeds new mono samples at 48 kHz and appends the output samples that became final to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        assert!(!self.closed, "o fluxo já foi encerrado");
        for piece in input.chunks(self.block * self.hop) {
            self.audio.extend_from_slice(piece);
            let total = self.audio_base + self.audio.len();
            let computable = if total >= self.n / 2 { (total - self.n / 2) / self.hop + 1 } else { 0 };
            self.advance(computable);
            let upto = (self.emitted * self.hop).saturating_sub(self.n / 2);
            self.release(upto, out);
            self.trim();
        }
    }

    /// Ends the stream and appends every remaining output sample to `out`.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.closed {
            return;
        }
        self.closed = true;
        let total = self.audio_base + self.audio.len();
        self.advance(total / self.hop + 1);
        while self.emitted < self.frames {
            self.synthesize();
        }
        self.release(total, out);
    }

    /// Fixed-size processing for an audio callback: `output` (as long as `input`) receives the stream's output
    /// delayed by [`Realtime::frame_latency`] samples, zeros before the first. Allocates nothing once the first call
    /// has sized its queue; every call must pass the same length.
    pub fn push_frame(&mut self, input: &[f32], output: &mut [f32]) {
        assert_eq!(input.len(), output.len());
        if self.fifo_frame != input.len() {
            assert_eq!(self.fifo_frame, 0, "push_frame com tamanho diferente do primeiro");
            self.fifo_frame = input.len();
            let delay = self.frame_latency(input.len());
            self.fifo = VecDeque::with_capacity(delay + 4 * input.len() + 2 * self.n);
            self.fifo.extend(std::iter::repeat_n(0.0, delay));
        }
        let mut out = std::mem::take(&mut self.out);
        out.clear();
        self.push(input, &mut out);
        self.fifo.extend(out.iter().copied());
        self.out = out;
        for o in output.iter_mut() {
            *o = self.fifo.pop_front().unwrap_or(0.0);
        }
    }

    /// The delay in samples of [`Realtime::push_frame`] with frames of `frame` samples: the least that keeps every
    /// output frame full. With 20 ms frames (960 samples) it is 2944 samples (61.3 ms) at look-ahead 2.
    pub fn frame_latency(&self, frame: usize) -> usize {
        let (n, hop, la) = (self.n, self.hop, self.lookahead);
        let mut worst = 0usize;
        // the release pattern repeats with the frames' phase against the hop; a few thousand pushes cover it
        for k in 1..=4 * hop.max(frame) / frame.max(1) + 64 {
            let total = k * frame;
            let computable = if total >= n / 2 { (total - n / 2) / hop + 1 } else { 0 };
            let emitted = computable.saturating_sub(la);
            let released = (emitted * hop).saturating_sub(n / 2).min(total);
            worst = worst.max(total - released);
        }
        worst
    }

    fn advance(&mut self, limit: usize) {
        while self.frames < limit {
            let count = self.block.min(limit - self.frames);
            for i in 0..count {
                self.analyze(self.frames + i, i);
            }
            let bins = self.bins;
            self.net.run(
                &self.pool,
                &mut self.scratch,
                &self.block_spec[..count * bins],
                count,
                Some(&mut self.state),
                &mut self.block_raw[..count * bins * 4],
            );
            self.state.pos += count;
            for i in 0..count {
                let at = (self.frames + i) % self.ring;
                self.raw_ring[at * bins * 4..(at + 1) * bins * 4]
                    .copy_from_slice(&self.block_raw[i * bins * 4..(i + 1) * bins * 4]);
                self.spec_ring[at * bins..(at + 1) * bins].copy_from_slice(&self.block_spec[i * bins..(i + 1) * bins]);
            }
            self.frames += count;
            while self.emitted + self.lookahead < self.frames {
                self.synthesize();
            }
        }
    }

    /// The spectrum of frame `frame` into slot `slot` of the block: its window of the audio, zeros where none.
    fn analyze(&mut self, frame: usize, slot: usize) {
        let start = (frame * self.hop) as isize - (self.n / 2) as isize;
        let total = (self.audio_base + self.audio.len()) as isize;
        for (i, s) in self.segment.iter_mut().enumerate() {
            let at = start + i as isize;
            *s = if at >= self.audio_base as isize && at < total {
                self.audio[at as usize - self.audio_base]
            } else {
                0.0
            };
        }
        let bins = self.bins;
        let window = &self.net.window;
        self.fft.forward(&self.segment, window, &mut self.block_spec[slot * bins..(slot + 1) * bins]);
    }

    fn synthesize(&mut self) {
        let index = self.emitted;
        let last = self.frames - 1;
        let source = if index <= last { (index + self.lookahead).min(last) } else { index };
        let bins = self.bins;
        let (rs, ss) = (source % self.ring, index % self.ring);
        self.net.correct(
            &self.raw_ring[rs * bins * 4..(rs + 1) * bins * 4],
            &self.spec_ring[ss * bins..(ss + 1) * bins],
            &mut self.corrected,
        );
        self.fft.inverse(&self.corrected, &self.net.window, &mut self.frame);
        let start = index * self.hop - self.ola_base;
        assert!(start + self.n <= self.ola.len(), "buffer de síntese pequeno demais");
        for i in 0..self.n {
            let w = self.net.window[i];
            self.ola[start + i] += self.frame[i];
            self.env[start + i] += w * w;
        }
        self.emitted += 1;
    }

    fn release(&mut self, upto: usize, out: &mut Vec<f32>) {
        let total = self.audio_base + self.audio.len();
        let (start, stop) = (self.released, upto.min(total));
        if stop <= start {
            return;
        }
        for s in start..stop {
            let at = s + self.n / 2 - self.ola_base;
            let delta = self.ola[at] / self.env[at].max(1e-11);
            out.push(self.audio[s - self.audio_base] + delta);
        }
        self.released = stop;
    }

    fn trim(&mut self) {
        // never past the next frame's first slot: frames still to come write from `emitted * hop` on
        let keep_from = (self.released + self.n / 2).min(self.emitted * self.hop);
        if keep_from > self.ola_base {
            let shift = keep_from - self.ola_base;
            let len = self.ola.len();
            self.ola.copy_within(shift.., 0);
            self.env.copy_within(shift.., 0);
            self.ola[len - shift..].fill(0.0);
            self.env[len - shift..].fill(0.0);
            self.ola_base = keep_from;
        }
        let needed = self.released.min((self.frames * self.hop).saturating_sub(self.n / 2));
        if needed > self.audio_base {
            self.audio.drain(..needed - self.audio_base);
            self.audio_base = needed;
        }
    }
}
