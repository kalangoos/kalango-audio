//! Small DSP toolbox: biquad filters and the per-block processors.
//!
//! Everything here is built from standard, public DSP material: RBJ "Audio EQ Cookbook" biquads,
//! a textbook feed-forward compressor, a look-ahead peak limiter, mid/side widening, spectral
//! subtraction and FFT convolution. The constants are our own choices and are explained where used.
use crate::model::{Block, Params};
use std::f32::consts::PI;

pub type Frame = [f32; 2];
/// Session sample rate.
pub const SR: f32 = 48000.0;

#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b0: f32, b1: f32, b2: f32, a1: f32, a2: f32,
    z1: [f32; 2], z2: [f32; 2],
}

#[derive(Clone, Copy)]
pub enum Shape { Peak, LowShelf, HighShelf, LowPass, Notch, HighPass, BandPass }

impl Biquad {
    pub fn new(shape: Shape, freq: f32, q: f32, gain_db: f32) -> Self {
        let mut b = Biquad::default();
        b.set(shape, freq, q, gain_db);
        b
    }
    /// RBJ Audio EQ Cookbook coefficients (Q form; for the shelves Q = 1/sqrt(2) is the S = 1 slope).
    pub fn set(&mut self, shape: Shape, freq: f32, q: f32, gain_db: f32) {
        let f = freq.clamp(10.0, SR * 0.45);
        let w0 = 2.0 * PI * f / SR;
        let (sw, cw) = (w0.sin(), w0.cos());
        let a = 10f32.powf(gain_db / 40.0);
        let alpha = sw / (2.0 * q.max(0.05));
        let (b0, b1, b2, a0, a1, a2);
        match shape {
            Shape::Peak => {
                b0 = 1.0 + alpha * a; b1 = -2.0 * cw; b2 = 1.0 - alpha * a;
                a0 = 1.0 + alpha / a; a1 = -2.0 * cw; a2 = 1.0 - alpha / a;
            }
            Shape::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                b0 = a * ((a + 1.0) - (a - 1.0) * cw + s);
                b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cw);
                b2 = a * ((a + 1.0) - (a - 1.0) * cw - s);
                a0 = (a + 1.0) + (a - 1.0) * cw + s;
                a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cw);
                a2 = (a + 1.0) + (a - 1.0) * cw - s;
            }
            Shape::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                b0 = a * ((a + 1.0) + (a - 1.0) * cw + s);
                b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cw);
                b2 = a * ((a + 1.0) + (a - 1.0) * cw - s);
                a0 = (a + 1.0) - (a - 1.0) * cw + s;
                a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cw);
                a2 = (a + 1.0) - (a - 1.0) * cw - s;
            }
            Shape::LowPass => {
                b0 = (1.0 - cw) / 2.0; b1 = 1.0 - cw; b2 = (1.0 - cw) / 2.0;
                a0 = 1.0 + alpha; a1 = -2.0 * cw; a2 = 1.0 - alpha;
            }
            Shape::HighPass => {
                b0 = (1.0 + cw) / 2.0; b1 = -(1.0 + cw); b2 = (1.0 + cw) / 2.0;
                a0 = 1.0 + alpha; a1 = -2.0 * cw; a2 = 1.0 - alpha;
            }
            Shape::BandPass => {
                b0 = alpha; b1 = 0.0; b2 = -alpha;
                a0 = 1.0 + alpha; a1 = -2.0 * cw; a2 = 1.0 - alpha;
            }
            Shape::Notch => {
                b0 = 1.0; b1 = -2.0 * cw; b2 = 1.0;
                a0 = 1.0 + alpha; a1 = -2.0 * cw; a2 = 1.0 - alpha;
            }
        }
        self.b0 = b0 / a0; self.b1 = b1 / a0; self.b2 = b2 / a0;
        self.a1 = a1 / a0; self.a2 = a2 / a0;
    }
    /// Transposed direct form II, one state per channel.
    #[inline]
    pub fn run(&mut self, ch: usize, x: f32) -> f32 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }
}

/// Butterworth Q for second-order sections.
const BUTTERWORTH_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Bass control: RBJ low shelf at 120 Hz, S = 1.
pub const BASS_CORNER: f32 = 120.0;
/// Treble control: RBJ high shelf at 8 kHz, S = 1.
pub const TREBLE_CORNER: f32 = 8000.0;
/// Range of the bass, treble and graphic-EQ sliders: slider 0..1 maps to -12..+12 dB, 0.5 is flat.
const SLIDER_RANGE_DB: f32 = 24.0;
/// Graphic EQ: the ISO 266 octave-band centres.
pub const EQ_FREQS: [f32; 10] = [31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
/// One-octave bandwidth for the graphic EQ bands.
const EQ_Q: f32 = std::f32::consts::SQRT_2;

pub fn db_to_lin(db: f32) -> f32 { 10f32.powf(db / 20.0) }
pub fn lin_to_db(x: f32) -> f32 { 20.0 * x.max(1e-9).log10() }

/// One-pole smoothing coefficient for a time constant in seconds at the session rate.
fn one_pole(seconds: f32) -> f32 { (-1.0 / (seconds.max(1e-5) * SR)).exp() }

/// Per-block DSP state.
#[derive(Default)]
pub struct Dsp {
    filters: Vec<Biquad>,
    key: Vec<f32>,        // parameter snapshot that the filters were built for
    last: [f32; 2],       // click-removal history
    phase: f32,           // tone generator
    limiter: Option<Box<Limiter>>,
    comp: Option<Box<Compressor>>,
    /// status for the window (time-shift lag in seconds, noise-reduction learned seconds, speech probability)
    pub aux: f64,
    seen: f64,            // last seen value of a button counter
    dl: std::collections::VecDeque<Frame>, // delay line / time-shift history
    lag: usize,
    duck_hold: usize,
    duck_gain: f32,
    nr: Option<Box<Denoise>>,
    rn: Option<[Box<nnnoiseless::DenoiseState<'static>>; 2]>,
    rn_in: [VecDeque<f32>; 2],
    rn_out: [VecDeque<f32>; 2],
    fir: Option<Box<Fir>>,
    fir_path: String,
    /// text for the window (FIR impulse info or an error)
    pub note: String,
    widen: Option<Box<Widener>>,
    plugin: Option<crate::ladspa::Instance>,
    plugin_key: String,
    /// Mixer fade state per track (0 = full level, 1 = faded out)
    pub fade_out: [f32; 5],
}

impl Dsp {
    /// Process `buf` in place for effect blocks; unknown kinds pass through.
    pub fn process(&mut self, b: &Block, buf: &mut [Frame]) {
        let p = &b.p;
        match b.kind.as_str() {
            "volume" => {
                let g = gain_total(p);
                if g <= 1.0 {
                    // plain gain; the limiter only runs when the boost takes the total above 100 %
                    self.limiter = None;
                    for f in buf.iter_mut() { f[0] *= g; f[1] *= g; }
                } else {
                    self.limiter.get_or_insert_with(|| Box::new(Limiter::new())).process(buf, g);
                }
            }
            "balance" => apply_balance(buf, p.num("balance", 0.0) as f32),
            "pan" => apply_pan(buf, p.num("pan", 0.0) as f32),
            "voice_denoise" => self.rnnoise(buf, p),
            "convolution" => self.fir_filter(buf, p),
            "ladspa_plugin" => self.plugin_fx(buf, p),
            "tone_control" => {
                let key = vec![p.num("bass", 0.5) as f32, p.num("treble", 0.5) as f32];
                if self.key != key {
                    self.filters = vec![
                        Biquad::new(Shape::LowShelf, BASS_CORNER, BUTTERWORTH_Q, (key[0] - 0.5) * SLIDER_RANGE_DB),
                        Biquad::new(Shape::HighShelf, TREBLE_CORNER, BUTTERWORTH_Q, (key[1] - 0.5) * SLIDER_RANGE_DB),
                    ];
                    self.key = key;
                }
                self.run_filters(buf);
            }
            "graphic_eq" => {
                let key: Vec<f32> = (0..10).map(|i| p.num(&format!("band{i}"), 0.5) as f32).collect();
                if self.key != key {
                    self.filters = (0..10).map(|i| Biquad::new(Shape::Peak, EQ_FREQS[i], EQ_Q, (key[i] - 0.5) * SLIDER_RANGE_DB)).collect();
                    self.key = key;
                }
                self.run_filters(buf);
            }
            "low_pass" => {
                // second-order Butterworth low-pass
                let key = vec![p.num("freq", 10000.0) as f32];
                if self.key != key {
                    self.filters = vec![Biquad::new(Shape::LowPass, key[0], BUTTERWORTH_Q, 0.0)];
                    self.key = key;
                }
                self.run_filters(buf);
            }
            "hum_removal" => {
                // Narrow peaking cuts (Q 30) at the mains frequency and its harmonics up to the 8th.
                // `amount` (1..8) sets the depth: 5 dB per step, so the default 3 cuts 15 dB.
                let key = vec![p.num("base", 50.0) as f32, p.num("amount", 3.0) as f32];
                if self.key != key {
                    let depth = (5.0 * key[1]).clamp(0.0, 48.0);
                    self.filters = (1..=HUM_HARMONICS)
                        .map(|n| key[0] * n as f32)
                        .filter(|f| *f < SR * 0.45)
                        .map(|f| Biquad::new(Shape::Peak, f, HUM_Q, -depth))
                        .collect();
                    self.key = key;
                }
                self.run_filters(buf);
            }
            "loudness" => {
                // Loudness: a gentle bass/treble contour, a slow 3:1 compressor, make-up gain and a
                // -1 dBFS look-ahead limiter so the louder result never clips.
                let amount = (p.num("amount", 0.0) as f32).clamp(0.0, 1.0);
                let speed = (p.num("speed", 0.0) as f32).clamp(0.0, 1.0);
                let makeup = (p.num("makeup", 1.0) as f32).clamp(0.0, 4.0);
                let key = vec![amount];
                if self.key != key {
                    self.filters = vec![
                        Biquad::new(Shape::LowShelf, 100.0, BUTTERWORTH_Q, 3.0 + 6.0 * amount),
                        Biquad::new(Shape::HighShelf, 10000.0, BUTTERWORTH_Q, 2.0 + 4.0 * amount),
                    ];
                    self.key = key;
                }
                self.run_filters(buf);
                let settings = CompSettings { threshold_db: -20.0 - 12.0 * amount, ratio: 3.0, speed, makeup: makeup * db_to_lin(6.0 * amount) };
                self.comp.get_or_insert_with(|| Box::new(Compressor::new())).process(buf, &settings);
                self.limiter.get_or_insert_with(|| Box::new(Limiter::new())).process(buf, 1.0);
            }
            "compressor" => {
                let settings = CompSettings::from_amount(
                    p.num("amount", 1.0) as f32,
                    p.num("speed", 0.0) as f32,
                    p.num("makeup", 1.0) as f32,
                );
                self.comp.get_or_insert_with(|| Box::new(Compressor::new())).process(buf, &settings);
            }
            "parametric_eq" => {
                let n = (p.num("bands", 3.0) as usize).clamp(0, 16);
                let mut key: Vec<f32> = vec![n as f32];
                for i in 0..n { for k in ["type", "freq", "gain", "q", "on"] { key.push(p.num(&format!("b{i}_{k}"), 0.0) as f32); } }
                if self.key != key {
                    self.filters = (0..n).filter(|i| p.num(&format!("b{i}_on"), 1.0) > 0.5).map(|i| {
                        let shape = match p.num(&format!("b{i}_type"), 0.0) as i32 { 1 => Shape::LowShelf, 2 => Shape::HighShelf, 3 => Shape::LowPass, 4 => Shape::HighPass, 5 => Shape::BandPass, 6 => Shape::Notch, _ => Shape::Peak };
                        Biquad::new(shape, p.num(&format!("b{i}_freq"), 1000.0) as f32, (p.num(&format!("b{i}_q"), 1.0) as f32).max(0.1), p.num(&format!("b{i}_gain"), 0.0) as f32)
                    }).collect();
                    self.key = key;
                }
                self.run_filters(buf);
            }
            "high_pass" | "band_pass" | "high_shelf" | "low_shelf" => {
                let key = vec![p.num("freq", 1000.0) as f32, p.num("gain", 0.0) as f32, p.num("q", 0.707) as f32];
                if self.key != key {
                    let shape = match b.kind.as_str() { "high_pass" => Shape::HighPass, "band_pass" => Shape::BandPass, "high_shelf" => Shape::HighShelf, _ => Shape::LowShelf };
                    self.filters = vec![Biquad::new(shape, key[0], key[2].max(0.1), key[1])];
                    self.key = key;
                }
                self.run_filters(buf);
            }
            "delay" => {
                // Plain delay line: `delay` milliseconds, rounded to the nearest frame.
                let frames = (p.num("delay", 0.0).max(0.0) * SR as f64 / 1000.0).round() as usize;
                while self.dl.len() < frames { self.dl.push_back([0.0; 2]); }
                while self.dl.len() > frames { self.dl.pop_front(); }
                if frames > 0 {
                    for f in buf.iter_mut() { self.dl.push_back(*f); *f = self.dl.pop_front().unwrap_or([0.0; 2]); }
                }
            }
            "time_shift" => self.time_shift(buf, p),
            "noise_reduction" => self.denoise(buf, p),
            "channels" => {
                let m = p.num("mode", 0.0) as i32;
                for f in buf.iter_mut() {
                    let (l, r) = (f[0], f[1]);
                    *f = match m {
                        1 => [r, l],
                        2 => [l, l],
                        3 => [r, r],
                        4 => { let x = 0.5 * (l + r); [x, x] }
                        5 => [0.0, r],
                        6 => [l, 0.0],
                        _ => [l, r],
                    };
                }
            }
            "stereo_widen" => self.widen(buf, p.num("crossover", 500.0) as f32),
            "click_removal" => {
                // Slew-rate limiter: a sample may move at most `0.5 / sens` (full scale = 1) from the
                // previous output sample. Music never moves that fast at 48 kHz; clicks and pops do.
                let sens = (p.num("sens", 3.0) as f32).max(0.5);
                let max_step = 0.5 / sens;
                for f in buf.iter_mut() {
                    for c in 0..2 {
                        let step = (f[c] - self.last[c]).clamp(-max_step, max_step);
                        f[c] = self.last[c] + step;
                        self.last[c] = f[c];
                    }
                }
            }
            _ => {}
        }
    }

    /// Pause and rewind live audio: keeps up to thirty minutes of history and plays it `lag` frames behind live.
    fn time_shift(&mut self, buf: &mut [Frame], p: &Params) {
        const MAX: usize = 48000 * 60 * 30; // 30 minutes of stereo f32 is about 690 MB at most
        let n = buf.len();
        let seq = p.num("jump_seq", 0.0) + p.num("live_seq", 0.0) * 1000.0;
        if seq != self.seen {
            self.seen = seq;
            let j = (p.num("jump_s", 0.0) * SR as f64) as i64;
            if p.num("jump_live", 0.0) > 0.5 { self.lag = 0; }
            else { self.lag = (self.lag as i64 + j).clamp(0, (self.dl.len().saturating_sub(n)) as i64) as usize; }
        }
        for f in buf.iter() { self.dl.push_back(*f); }
        while self.dl.len() > MAX + n { self.dl.pop_front(); }
        if p.num("paused", 0.0) > 0.5 {
            self.lag = (self.lag + n).min(self.dl.len().saturating_sub(n));
            for f in buf.iter_mut() { *f = [0.0; 2]; }
        } else {
            let len = self.dl.len();
            for (i, f) in buf.iter_mut().enumerate() {
                *f = if self.lag + n <= len { self.dl[len - self.lag - n + i] } else { [0.0; 2] };
            }
        }
        self.aux = self.lag as f64 / SR as f64;
    }

    /// Ducking: the overlay is heard while its peak is at or above `thr_db`; at other times it is muted.
    /// While it is heard the other sources move to `ducked` (linear gain). Once the overlay goes quiet
    /// the duck is held for 0.5 s, and the gain moves in linear ramps of 50 ms for the full range.
    pub fn duck(&mut self, overlay: &[Frame], others: &[Frame], thr_db: f32, ducked: f32) -> Vec<Frame> {
        const HOLD_S: f32 = 0.5;
        const RAMP_S: f32 = 0.05;
        let peak = overlay.iter().fold(0f32, |m, f| m.max(f[0].abs()).max(f[1].abs()));
        if lin_to_db(peak) >= thr_db { self.duck_hold = (SR * HOLD_S) as usize; } else { self.duck_hold = self.duck_hold.saturating_sub(overlay.len()); }
        let trig = self.duck_hold > 0;
        let target = if trig { ducked } else { 1.0 };
        let step = 1.0 / (SR * RAMP_S);
        let mut out = Vec::with_capacity(overlay.len());
        for i in 0..overlay.len() {
            if self.duck_gain < target { self.duck_gain = (self.duck_gain + step).min(target); } else { self.duck_gain = (self.duck_gain - step).max(target); }
            let o = if trig { overlay[i] } else { [0.0; 2] };
            out.push([o[0] + others[i][0] * self.duck_gain, o[1] + others[i][1] * self.duck_gain]);
        }
        out
    }

    fn denoise(&mut self, buf: &mut [Frame], p: &Params) {
        if self.nr.is_none() { self.nr = Some(Box::new(Denoise::new())); }
        let learn_seq = p.num("learn_seq", 0.0);
        let reduction = p.num("reduction", 6.0) as f32;
        let nr = self.nr.as_mut().unwrap();
        if learn_seq != self.seen {
            self.seen = learn_seq;
            if nr.learning { nr.learning = false; } else { nr.start_learning(); }
        }
        nr.process(buf, db_to_lin(-reduction));
        self.aux = nr.learned_frames as f64 / SR as f64;
    }

    /// Speech denoise: RNNoise (nnnoiseless crate) on 480-sample frames, per channel or on a mono mix.
    fn rnnoise(&mut self, buf: &mut [Frame], p: &Params) {
        use nnnoiseless::{DenoiseState, FRAME_SIZE};
        if self.rn.is_none() {
            self.rn = Some([DenoiseState::new(), DenoiseState::new()]);
            for c in 0..2 { self.rn_in[c].clear(); self.rn_out[c] = VecDeque::from(vec![0.0; FRAME_SIZE]); }
        }
        let mono = p.num("mono", 0.0) > 0.5;
        let rn = self.rn.as_mut().unwrap();
        let chans = if mono { 1 } else { 2 };
        let mut vad = 0.0f32;
        for c in 0..chans {
            // RNNoise expects 16-bit sample scale
            for f in buf.iter() { self.rn_in[c].push_back(if mono { 0.5 * (f[0] + f[1]) } else { f[c] } * 32768.0); }
            while self.rn_in[c].len() >= FRAME_SIZE {
                let inp: Vec<f32> = self.rn_in[c].drain(..FRAME_SIZE).collect();
                let mut out = vec![0.0f32; FRAME_SIZE];
                vad = vad.max(rn[c].process_frame(&mut out, &inp));
                self.rn_out[c].extend(out.iter().map(|x| x / 32768.0));
            }
        }
        for f in buf.iter_mut() {
            let l = self.rn_out[0].pop_front().unwrap_or(0.0);
            let r = if mono { l } else { self.rn_out[1].pop_front().unwrap_or(0.0) };
            *f = [l, r];
        }
        self.aux = vad as f64;
    }

    /// Stereo widener for mono material; real stereo passes untouched.
    fn widen(&mut self, buf: &mut [Frame], xover: f32) {
        if buf.iter().any(|f| f[0] != f[1]) { self.widen = None; return; }
        self.widen.get_or_insert_with(|| Box::new(Widener::new())).process(buf, xover);
    }

    /// Effect plug-in (LADSPA): started when chosen, its controls follow the block's settings.
    fn plugin_fx(&mut self, buf: &mut [Frame], p: &Params) {
        let key = p.text("plugin", "");
        if key != self.plugin_key {
            self.plugin_key = key.clone();
            self.plugin = None;
            self.note.clear();
            if !key.is_empty() {
                match crate::ladspa::find(&key).ok_or_else(|| "the plug-in was not found".to_string()).and_then(|i| crate::ladspa::Instance::new(&i, SR)) {
                    Ok(inst) => { self.note = "running".into(); self.plugin = Some(inst); }
                    Err(e) => self.note = format!("error: {e}"),
                }
            }
        }
        if let (Some(inst), Some(info)) = (self.plugin.as_mut(), crate::ladspa::find_cached(&key)) {
            for (i, c) in info.controls.iter().enumerate() { inst.set_control(i, p.num(&format!("c{}", c.port), c.default as f64) as f32); }
        }
        if let Some(inst) = self.plugin.as_mut() { inst.process(buf); }
    }

    /// Convolution: input gain in dB, then FFT convolution with the chosen impulse response.
    fn fir_filter(&mut self, buf: &mut [Frame], p: &Params) {
        let path = p.text("ir", "");
        if path != self.fir_path || (self.fir.is_none() && !path.is_empty() && self.note.is_empty()) {
            self.fir_path = path.clone();
            self.fir = None;
            self.note.clear();
            if !path.is_empty() {
                match Fir::load(&path) {
                    Ok(f) => { self.note = f.info(); self.fir = Some(Box::new(f)); }
                    Err(e) => self.note = format!("error: {e}"),
                }
            }
        }
        let g = db_to_lin(p.num("pregain", 0.0) as f32);
        for f in buf.iter_mut() { f[0] *= g; f[1] *= g; }
        if let Some(fir) = self.fir.as_mut() { fir.process(buf); }
    }

    fn run_filters(&mut self, buf: &mut [Frame]) {
        for f in buf.iter_mut() {
            for c in 0..2 {
                let mut x = f[c];
                for bq in self.filters.iter_mut() { x = bq.run(c, x); }
                f[c] = x;
            }
        }
    }

    pub fn tone(&mut self, freq: f32, level: f32, out: &mut [Frame]) {
        for f in out.iter_mut() {
            let s = (self.phase * 2.0 * PI).sin() * level;
            self.phase = (self.phase + freq / SR).fract();
            *f = [s, s];
        }
    }
}

/// Hum removal: number of harmonics cut (the fundamental counts as the first).
const HUM_HARMONICS: usize = 8;
/// Hum removal: Q of each cut (about 1.7 Hz wide at 50 Hz).
const HUM_Q: f32 = 30.0;

// ---------------- Look-ahead peak limiter ----------------

/// Look-ahead of the limiter: 5 ms.
const LIMITER_LOOKAHEAD_S: f32 = 0.005;
/// Output ceiling of the limiter: -1 dBFS.
const LIMITER_CEILING_DB: f32 = -1.0;
/// Release time constant of the limiter.
const LIMITER_RELEASE_S: f32 = 0.08;

/// Look-ahead peak limiter.
///
/// For every incoming frame we compute the gain that would keep it under the ceiling. A sliding
/// minimum over the look-ahead window turns that into a "hold" that starts before each peak; the
/// hold is released with an exponential curve and then averaged over the same window, so the gain
/// ramps down smoothly and has reached the needed value when the peak leaves the delay line.
pub struct Limiter {
    len: usize,
    delay: VecDeque<Frame>,
    mins: VecDeque<(u64, f32)>, // monotonic queue for the sliding minimum
    ramp: VecDeque<f32>,        // released gains inside the averaging window
    ramp_sum: f64,
    released: f32,
    t: u64,
}

impl Limiter {
    pub fn new() -> Self {
        let len = ((LIMITER_LOOKAHEAD_S * SR) as usize).max(2);
        Limiter {
            len,
            delay: VecDeque::from(vec![[0.0; 2]; len - 1]),
            mins: VecDeque::new(),
            ramp: VecDeque::from(vec![1.0; len]),
            ramp_sum: len as f64,
            released: 1.0,
            t: 0,
        }
    }

    /// Multiplies by `pre_gain`, then limits the result to the ceiling. Adds `len - 1` frames of latency.
    pub fn process(&mut self, buf: &mut [Frame], pre_gain: f32) {
        let ceiling = db_to_lin(LIMITER_CEILING_DB);
        let release = 1.0 - one_pole(LIMITER_RELEASE_S);
        for f in buf.iter_mut() {
            let x = [f[0] * pre_gain, f[1] * pre_gain];
            let peak = x[0].abs().max(x[1].abs());
            let need = if peak > ceiling { ceiling / peak } else { 1.0 };
            // sliding minimum of `need` over the last `len` frames
            while self.mins.back().is_some_and(|m| m.1 >= need) { self.mins.pop_back(); }
            self.mins.push_back((self.t, need));
            while self.mins.front().is_some_and(|m| m.0 + self.len as u64 <= self.t) { self.mins.pop_front(); }
            let hold = self.mins.front().map(|m| m.1).unwrap_or(1.0);
            self.t += 1;
            // instant attack, exponential release
            self.released = if hold < self.released { hold } else { self.released + (hold - self.released) * release };
            // moving average over the window gives the smooth attack ramp
            self.ramp_sum += self.released as f64 - self.ramp.pop_front().unwrap_or(1.0) as f64;
            self.ramp.push_back(self.released);
            let g = ((self.ramp_sum / self.len as f64) as f32).min(1.0);
            self.delay.push_back(x);
            let d = self.delay.pop_front().unwrap_or([0.0; 2]);
            // the average is never above the needed gain; the clamp only guards rounding
            *f = [(d[0] * g).clamp(-ceiling, ceiling), (d[1] * g).clamp(-ceiling, ceiling)];
        }
    }
}

impl Default for Limiter { fn default() -> Self { Self::new() } }

// ---------------- Compressor ----------------

/// Look-ahead of the compressor: 2 ms, so the attack has started when a transient comes out.
const COMP_LOOKAHEAD_S: f32 = 0.002;
/// Averaging time of the compressor's RMS detector.
const COMP_RMS_S: f32 = 0.005;
/// Soft-knee width of the compressor.
const COMP_KNEE_DB: f32 = 6.0;

pub struct CompSettings {
    pub threshold_db: f32,
    pub ratio: f32,
    /// 0 = slow (25 ms attack, 400 ms release), 1 = fast (1 ms attack, 50 ms release)
    pub speed: f32,
    /// linear output gain
    pub makeup: f32,
}

impl CompSettings {
    /// Compressor block mapping: `amount` 0..1 moves the threshold from 0 to -30 dBFS and the ratio
    /// from 1:1 (off) to 6:1; `speed` 0..1 picks the time constants; `makeup` is a linear gain.
    pub fn from_amount(amount: f32, speed: f32, makeup: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        CompSettings { threshold_db: -30.0 * amount, ratio: 1.0 + 5.0 * amount, speed: speed.clamp(0.0, 1.0), makeup: makeup.clamp(0.0, 4.0) }
    }
    fn attack_s(&self) -> f32 { 0.025 * (0.001f32 / 0.025).powf(self.speed) }
    fn release_s(&self) -> f32 { 0.4 * (0.05f32 / 0.4).powf(self.speed) }
}

/// Feed-forward compressor: RMS detector, soft-knee static curve in dB, attack/release smoothing of
/// the gain reduction, then make-up gain; the audio runs through a short look-ahead delay.
pub struct Compressor {
    mean_sq: f32,
    reduction_db: f32, // smoothed gain reduction, <= 0
    delay: VecDeque<Frame>,
}

impl Compressor {
    pub fn new() -> Self {
        let n = (COMP_LOOKAHEAD_S * SR) as usize;
        Compressor { mean_sq: 0.0, reduction_db: 0.0, delay: VecDeque::from(vec![[0.0; 2]; n]) }
    }

    /// Static curve: gain change in dB for a detector level in dB (soft knee).
    pub fn curve(level_db: f32, threshold_db: f32, ratio: f32) -> f32 {
        let slope = 1.0 / ratio.max(1.0) - 1.0;
        let over = level_db - threshold_db;
        if 2.0 * over < -COMP_KNEE_DB { 0.0 }
        else if 2.0 * over > COMP_KNEE_DB { slope * over }
        else { slope * (over + COMP_KNEE_DB / 2.0).powi(2) / (2.0 * COMP_KNEE_DB) }
    }

    pub fn process(&mut self, buf: &mut [Frame], s: &CompSettings) {
        let rms_c = one_pole(COMP_RMS_S);
        let atk = one_pole(s.attack_s());
        let rel = one_pole(s.release_s());
        for f in buf.iter_mut() {
            let sq = 0.5 * (f[0] * f[0] + f[1] * f[1]);
            self.mean_sq = rms_c * self.mean_sq + (1.0 - rms_c) * sq;
            let level_db = 10.0 * self.mean_sq.max(1e-12).log10();
            let target = Self::curve(level_db, s.threshold_db, s.ratio);
            let c = if target < self.reduction_db { atk } else { rel };
            self.reduction_db = c * self.reduction_db + (1.0 - c) * target;
            let g = db_to_lin(self.reduction_db) * s.makeup;
            self.delay.push_back(*f);
            let d = self.delay.pop_front().unwrap_or(*f);
            *f = [d[0] * g, d[1] * g];
        }
    }
}

impl Default for Compressor { fn default() -> Self { Self::new() } }

// ---------------- Balance, pan, mixer, volume ----------------

/// Balance: linear attenuation of the opposite side. +1 silences the left, -1 the right, 0 is unity.
pub fn apply_balance(buf: &mut [Frame], bal: f32) {
    let bal = bal.clamp(-1.0, 1.0);
    let (gl, gr) = if bal >= 0.0 { (1.0 - bal, 1.0) } else { (1.0, 1.0 + bal) };
    for f in buf.iter_mut() { f[0] *= gl; f[1] *= gr; }
}

/// Constant-power (sin/cos) gains for a position -1 (left) .. +1 (right); the centre gives -3 dB per side.
pub fn pan_gains(pos: f32) -> (f32, f32) {
    let theta = (pos.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
    (theta.cos(), theta.sin())
}

/// Stereo pan: each input channel is placed with the constant-power law. At centre the left input
/// sits hard left and the right input hard right (unity); panning right moves the left input across
/// (it reaches the middle at +0.5 and the right side at +1) while the right input stays put, and the
/// mirror image for panning left.
pub fn apply_pan(buf: &mut [Frame], pan: f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let pos_l = (-1.0 + 2.0 * pan.max(0.0)).min(1.0);
    let pos_r = (1.0 + 2.0 * pan.min(0.0)).max(-1.0);
    let (ll, lr) = pan_gains(pos_l);
    let (rl, rr) = pan_gains(pos_r);
    for f in buf.iter_mut() {
        let (l, r) = (f[0], f[1]);
        *f = [l * ll + r * rl, l * lr + r * rr];
    }
}

/// Mixer: up to five tracks, each with gain, balance, pan, mute, solo and a fade.
/// `fade_out` holds how far each track has faded (0 = full level, 1 = silent); it moves linearly
/// toward its target over one second, and keeps moving even for a muted track.
pub fn mix_tracks(tracks: &[Vec<Frame>], p: &Params, fade_out: &mut [f32; 5]) -> Vec<Frame> {
    const FADE_S: f32 = 1.0;
    let n = tracks.first().map(|t| t.len()).unwrap_or(0);
    let mut out = vec![[0f32; 2]; n];
    let solo = (0..5).any(|i| p.num(&format!("s{i}"), 0.0) > 0.5);
    for (i, t) in tracks.iter().take(5).enumerate() {
        let target = if p.num(&format!("fo{i}"), 0.0) > 0.5 { 1.0 } else { 0.0 };
        let step = t.len() as f32 / (SR * FADE_S);
        fade_out[i] = if fade_out[i] < target { (fade_out[i] + step).min(target) } else { (fade_out[i] - step).max(target) };
        let muted = p.num(&format!("m{i}"), 0.0) > 0.5 || (solo && p.num(&format!("s{i}"), 0.0) < 0.5);
        if muted { continue; }
        let g = p.num(&format!("g{i}"), 1.0) as f32 * (1.0 - fade_out[i]);
        let mut v = t.clone();
        apply_balance(&mut v, p.num(&format!("b{i}"), 0.0) as f32);
        apply_pan(&mut v, p.num(&format!("p{i}"), 0.0) as f32);
        for (o, f) in out.iter_mut().zip(&v) { o[0] += f[0] * g; o[1] += f[1] * g; }
    }
    out
}

/// Volume block: total gain is the slider (0..1) times the boost multiplier (1..4); mute silences it.
pub fn gain_total(p: &Params) -> f32 {
    if p.num("muted", 0.0) > 0.5 { return 0.0; }
    let gain = (p.num("gain", 1.0) as f32).clamp(0.0, 1.0);
    let boost = (p.num("boost", 1.0) as f32).clamp(1.0, 4.0);
    gain * boost
}

// ---------------- VU needle ----------------

/// dBFS (RMS) that reads 0 VU: the common -18 dBFS digital alignment.
pub const VU_REFERENCE_DBFS: f32 = -18.0;
/// Needle angle (degrees from vertical) at rest and at full scale.
const VU_SWEEP_DEG: f32 = 45.0;

/// Needle angle of the VU meter for an RMS level in dBFS.
///
/// The printed scale runs from -20 VU to +3 VU with 0 VU = -18 dBFS. Like a moving-coil meter the
/// deflection is proportional to the signal voltage: full scale (+3 VU) is the right stop, 0 VU sits
/// at about 71 % of the arc and -20 VU at about 7 %. The sweep is -45..+45 degrees.
pub fn vu_angle(db: f32) -> f32 {
    let vu = db - VU_REFERENCE_DBFS;
    let deflection = (10f32.powf((vu - 3.0) / 20.0)).clamp(0.0, 1.0);
    -VU_SWEEP_DEG + 2.0 * VU_SWEEP_DEG * deflection
}

// ---------------- Stereo widener ----------------

/// Delay of the widener's side signal (Haas range, well under the echo threshold).
const WIDEN_DELAY_S: f32 = 0.012;
/// Level of the side signal relative to the mid.
const WIDEN_WIDTH: f32 = 0.5;

/// Mid/side widener for mono input: the side channel is a delayed, high-passed copy of the input,
/// added to the left and subtracted from the right. Below the crossover the side is filtered away, so
/// bass stays centred; L + R is always exactly twice the input, so the result folds back to mono cleanly.
pub struct Widener {
    delay: VecDeque<f32>,
    hp: [Biquad; 2], // fourth-order high-pass: two Butterworth sections
    last_xover: f32,
}

impl Widener {
    fn new() -> Self {
        let n = (WIDEN_DELAY_S * SR) as usize;
        Widener { delay: VecDeque::from(vec![0.0; n]), hp: [Biquad::default(); 2], last_xover: -1.0 }
    }

    fn process(&mut self, buf: &mut [Frame], xover: f32) {
        if xover != self.last_xover {
            let f = xover.clamp(20.0, 2000.0);
            // fourth-order Butterworth as two sections (Q 0.5412 and 1.3066)
            self.hp = [Biquad::new(Shape::HighPass, f, 0.541_196_1, 0.0), Biquad::new(Shape::HighPass, f, 1.306_563, 0.0)];
            self.last_xover = xover;
        }
        let norm = 1.0 / (1.0 + WIDEN_WIDTH * WIDEN_WIDTH).sqrt();
        for f in buf.iter_mut() {
            let x = f[0];
            self.delay.push_back(x);
            let d = self.delay.pop_front().unwrap_or(0.0);
            let h = self.hp[0].run(0, d);
            let s = self.hp[1].run(0, h) * WIDEN_WIDTH;
            // scale so the loudness stays about the same once the side is added
            *f = [(x + s) * norm, (x - s) * norm];
        }
    }
}

// ---------------- FIR filter: convolution through the FFT ----------------

const FIR_N: usize = 4096;
pub const FIR_MAX_TAPS: usize = FIR_N - 480;

pub struct Fir {
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    h: Vec<Complex<f32>>,
    tail: [Vec<f32>; 2],
    taps: usize,
    file_rate: u32,
    resampled: bool,
    truncated: bool,
}

impl Fir {
    pub fn load(path: &str) -> Result<Fir, String> {
        let p = if let Some(r) = path.strip_prefix('~') { format!("{}/{}", std::env::var("HOME").unwrap_or_default(), r.trim_start_matches('/')) } else { path.to_string() };
        let mut r = hound::WavReader::open(&p).map_err(|e| format!("could not read the impulse response: {e}"))?;
        let spec = r.spec();
        let ch = spec.channels.max(1) as usize;
        let raw: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().filter_map(|x| x.ok()).collect(),
            hound::SampleFormat::Int => { let m = (1i64 << (spec.bits_per_sample - 1)) as f32; r.samples::<i32>().filter_map(|x| x.ok()).map(|x| x as f32 / m).collect() }
        };
        let mut ir: Vec<f32> = raw.chunks(ch).map(|c| c[0]).collect(); // first channel, used for both sides
        if ir.is_empty() { return Err("the impulse response is empty".into()); }
        let mut resampled = false;
        if spec.sample_rate != SR as u32 {
            // an impulse recorded at another rate is brought to the session rate (linear interpolation)
            let ratio = spec.sample_rate as f32 / SR;
            let n = (ir.len() as f32 / ratio) as usize;
            ir = (0..n).map(|i| { let x = i as f32 * ratio; let a = x as usize; let fr = x - a as f32; ir[a.min(ir.len() - 1)] * (1.0 - fr) + ir[(a + 1).min(ir.len() - 1)] * fr }).collect();
            resampled = true;
        }
        let truncated = ir.len() > FIR_MAX_TAPS;
        ir.truncate(FIR_MAX_TAPS);
        let mut pl = FftPlanner::new();
        let fft = pl.plan_fft_forward(FIR_N);
        let ifft = pl.plan_fft_inverse(FIR_N);
        let mut h: Vec<Complex<f32>> = ir.iter().map(|x| Complex::new(*x, 0.0)).collect();
        h.resize(FIR_N, Complex::new(0.0, 0.0));
        fft.process(&mut h);
        Ok(Fir { fft, ifft, h, tail: [vec![0.0; FIR_N], vec![0.0; FIR_N]], taps: ir.len(), file_rate: spec.sample_rate, resampled, truncated })
    }

    pub fn info(&self) -> String {
        let mut s = format!("{} taps, {:.1} ms", self.taps, self.taps as f32 * 1000.0 / SR);
        if self.resampled { s += &format!(" · ⚠ resampled from {} Hz", self.file_rate); }
        if self.truncated { s += &format!(" · cut to {} taps", FIR_MAX_TAPS); }
        s
    }

    /// Overlap-add convolution of up to 480 frames per call.
    pub fn process(&mut self, buf: &mut [Frame]) {
        let n = buf.len().min(480);
        for c in 0..2 {
            let mut x: Vec<Complex<f32>> = buf.iter().take(n).map(|f| Complex::new(f[c], 0.0)).collect();
            x.resize(FIR_N, Complex::new(0.0, 0.0));
            self.fft.process(&mut x);
            for (a, b) in x.iter_mut().zip(&self.h) { *a *= *b; }
            self.ifft.process(&mut x);
            for i in 0..FIR_N { self.tail[c][i] += x[i].re / FIR_N as f32; }
            for (i, f) in buf.iter_mut().take(n).enumerate() { f[c] = self.tail[c][i]; }
            self.tail[c].drain(..n);
            self.tail[c].extend(std::iter::repeat(0.0).take(n));
        }
    }
}

// ---------------- Noise reduction: spectral subtraction trained on a noise sample ----------------
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::collections::VecDeque;
use std::sync::Arc;

const NF: usize = 1024;
const HOP: usize = 512;
/// How long the noise profile is learned for.
pub const DENOISE_LEARN_S: f32 = 2.0;
/// Over-subtraction factor: a bin must be this many times the learned noise to pass unchanged-ish.
const DENOISE_OVERSUBTRACT: f32 = 2.0;

struct Chan { inq: VecDeque<f32>, ola: Vec<f32>, outq: VecDeque<f32>, prof: Vec<f32> }

/// Learns the average magnitude spectrum of the noise (for `DENOISE_LEARN_S` seconds), then applies
/// magnitude spectral subtraction with a gain floor set by the chosen reduction. 1024-point FFT,
/// sine window for analysis and synthesis, 50 % overlap.
pub struct Denoise {
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    win: Vec<f32>,
    ch: [Chan; 2],
    pub learning: bool,
    pub learned_frames: usize,
    prof_n: usize,
}

impl Denoise {
    fn new() -> Self {
        let mut pl = FftPlanner::new();
        let win = (0..NF).map(|i| (std::f32::consts::PI * (i as f32 + 0.5) / NF as f32).sin()).collect();
        let mk = || Chan { inq: VecDeque::new(), ola: vec![0.0; NF], outq: VecDeque::from(vec![0.0; NF]), prof: vec![0.0; NF / 2 + 1] };
        Denoise { fft: pl.plan_fft_forward(NF), ifft: pl.plan_fft_inverse(NF), win, ch: [mk(), mk()], learning: false, learned_frames: 0, prof_n: 0 }
    }
    fn start_learning(&mut self) {
        self.learning = true; self.learned_frames = 0; self.prof_n = 0;
        for c in self.ch.iter_mut() { for v in c.prof.iter_mut() { *v = 0.0; } }
    }
    fn process(&mut self, buf: &mut [Frame], floor: f32) {
        if self.learning {
            self.learned_frames += buf.len();
            if self.learned_frames >= (SR * DENOISE_LEARN_S) as usize { self.learning = false; }
        }
        for c in 0..2 {
            for f in buf.iter() { self.ch[c].inq.push_back(f[c]); }
            while self.ch[c].inq.len() >= NF {
                let mut frame: Vec<Complex<f32>> = (0..NF).map(|i| Complex::new(self.ch[c].inq[i] * self.win[i], 0.0)).collect();
                self.fft.process(&mut frame);
                let learned = self.prof_n > 0;
                if self.learning {
                    if c == 0 { self.prof_n += 1; }
                    let n = self.prof_n.max(1) as f32;
                    for k in 0..=NF / 2 { let m = frame[k].norm(); self.ch[c].prof[k] += (m - self.ch[c].prof[k]) / n; }
                } else if learned {
                    for k in 0..=NF / 2 {
                        let m = frame[k].norm().max(1e-9);
                        let nz = self.ch[c].prof[k] * DENOISE_OVERSUBTRACT;
                        let g = if m > nz { 1.0 - nz / m } else { 0.0 };
                        let g = g.max(floor);
                        frame[k] *= g;
                        if k > 0 && k < NF / 2 { frame[NF - k] *= g; }
                    }
                }
                self.ifft.process(&mut frame);
                for i in 0..NF { self.ch[c].ola[i] += frame[i].re / NF as f32 * self.win[i]; }
                for i in 0..HOP { let v = self.ch[c].ola[i]; self.ch[c].outq.push_back(v); }
                self.ch[c].ola.drain(..HOP);
                self.ch[c].ola.extend(std::iter::repeat(0.0).take(HOP));
                self.ch[c].inq.drain(..HOP);
            }
            for f in buf.iter_mut() { f[c] = self.ch[c].outq.pop_front().unwrap_or(0.0); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blk(kind: &str, set: &[(&str, f64)]) -> Block {
        let mut p = Params::default();
        for (k, v) in set { p.set(k, *v); }
        Block { id: 1, kind: kind.into(), x: 0.0, y: 0.0, enabled: true, outs: vec![], p }
    }
    fn sine(freq: f32, amp: f32, n: usize) -> Vec<Frame> {
        (0..n).map(|i| { let s = (2.0 * PI * freq * i as f32 / SR).sin() * amp; [s, s] }).collect()
    }
    fn run(kind: &str, set: &[(&str, f64)], input: Vec<Frame>) -> Vec<Frame> {
        let b = blk(kind, set);
        let mut d = Dsp::default();
        let mut out = vec![];
        for c in input.chunks(480) { let mut v = c.to_vec(); d.process(&b, &mut v); out.extend(v); }
        out
    }
    fn peak_tail(v: &[Frame]) -> f32 { v[v.len() / 2..].iter().fold(0.0, |m, f| m.max(f[0].abs())) }
    fn gain_db(kind: &str, set: &[(&str, f64)], freq: f32) -> f32 { lin_to_db(peak_tail(&run(kind, set, sine(freq, 0.1, 48000))) / 0.1) }
    fn rms(v: &[Frame]) -> f32 { (v.iter().map(|f| f[0] * f[0]).sum::<f32>() / v.len() as f32).sqrt() }

    #[test]
    fn balance_attenuates_the_opposite_side_linearly() {
        let o = run("balance", &[("balance", 0.5)], vec![[1.0, 1.0]; 480]);
        assert!((o[0][0] - 0.5).abs() < 1e-6 && (o[0][1] - 1.0).abs() < 1e-6);
        let o = run("balance", &[("balance", -0.25)], vec![[1.0, 1.0]; 480]);
        assert!((o[0][1] - 0.75).abs() < 1e-6 && (o[0][0] - 1.0).abs() < 1e-6);
        let o = run("balance", &[("balance", 1.0)], vec![[1.0, 1.0]; 480]);
        assert_eq!(o[0], [0.0, 1.0]);
    }

    #[test]
    fn pan_is_constant_power() {
        // centre leaves stereo untouched
        let o = run("pan", &[("pan", 0.0)], vec![[0.3, 0.6]; 480]);
        assert!((o[0][0] - 0.3).abs() < 1e-6 && (o[0][1] - 0.6).abs() < 1e-6);
        // at +0.5 the left input sits in the middle: -3 dB on each side, power kept
        let o = run("pan", &[("pan", 0.5)], vec![[1.0, 0.0]; 480]);
        assert!((o[0][0] - 0.70710677).abs() < 1e-5 && (o[0][1] - 0.70710677).abs() < 1e-5);
        assert!((o[0][0] * o[0][0] + o[0][1] * o[0][1] - 1.0).abs() < 1e-5);
        // hard right moves the left input fully right
        let o = run("pan", &[("pan", 1.0)], vec![[1.0, 0.0]; 480]);
        assert!(o[0][0].abs() < 1e-6 && (o[0][1] - 1.0).abs() < 1e-6);
        // and the mirror image
        let o = run("pan", &[("pan", -0.5)], vec![[0.0, 1.0]; 480]);
        assert!((o[0][0] - 0.70710677).abs() < 1e-5 && (o[0][1] - 0.70710677).abs() < 1e-5);
        // any position keeps the power of a single channel
        for i in -10..=10 {
            let (a, b) = pan_gains(i as f32 / 10.0);
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn volume_gain_mute_and_boost_limiter() {
        let mut p = Params::default();
        p.set("gain", 0.5); p.set("boost", 4.0);
        assert!((gain_total(&p) - 2.0).abs() < 1e-6);
        p.set("muted", 1.0);
        assert_eq!(gain_total(&p), 0.0);
        // plain gain at or below 100 %
        let o = run("volume", &[("gain", 0.5), ("boost", 1.0)], sine(440.0, 0.8, 4800));
        assert!((peak_tail(&o) - 0.4).abs() < 0.01);
        // 200 % total: the limiter holds the output at -1 dBFS
        let ceiling = db_to_lin(-1.0);
        let o = run("volume", &[("gain", 0.5), ("boost", 4.0)], sine(440.0, 0.9, 9600));
        assert!(o.iter().all(|f| f[0].abs() <= ceiling + 1e-4), "limiter let a peak through");
        assert!(peak_tail(&o) > ceiling - 0.02, "limiter should sit at the ceiling, got {}", peak_tail(&o));
        // a quiet signal below the ceiling is boosted cleanly (only delayed by the look-ahead)
        let o = run("volume", &[("gain", 1.0), ("boost", 2.0)], sine(440.0, 0.2, 9600));
        assert!((peak_tail(&o) - 0.4).abs() < 0.005);
    }

    #[test]
    fn limiter_catches_a_sudden_peak_and_recovers() {
        let mut l = Limiter::new();
        let mut buf = vec![[0.1f32; 2]; 48000];
        buf[2000] = [1.0, -1.0];
        l.process(&mut buf, 1.0);
        let ceiling = db_to_lin(-1.0);
        assert!(buf.iter().all(|f| f[0].abs() <= ceiling + 1e-5 && f[1].abs() <= ceiling + 1e-5));
        // the spike comes out after the look-ahead delay, already limited
        let lat = (LIMITER_LOOKAHEAD_S * SR) as usize - 1;
        assert!((buf[2000 + lat][0] - ceiling).abs() < 1e-3, "spike {}", buf[2000 + lat][0]);
        // after several release time constants the steady level is back
        assert!((buf[47999][0] - 0.1).abs() < 1e-3, "recovered to {}", buf[47999][0]);
    }

    #[test]
    fn eq_bands_sit_on_iso_centres_and_reach_twelve_db() {
        assert_eq!(EQ_FREQS[0], 31.5);
        assert_eq!(EQ_FREQS[5], 1000.0);
        let g = gain_db("graphic_eq", &[("band5", 1.0)], 1000.0);
        assert!((g - 12.0).abs() < 0.5, "gain was {g} dB");
        let g = gain_db("graphic_eq", &[("band7", 0.0)], 4000.0);
        assert!((g + 12.0).abs() < 0.5, "cut was {g} dB");
        // one octave away the band has dropped a lot
        let g = gain_db("graphic_eq", &[("band5", 1.0)], 4000.0);
        assert!(g < 3.0, "two octaves off still {g} dB");
        let flat = run("graphic_eq", &[], sine(1000.0, 0.1, 24000));
        assert!((peak_tail(&flat) - 0.1).abs() < 0.002);
    }

    #[test]
    fn hum_removal_cuts_50hz_and_its_harmonics() {
        let g = gain_db("hum_removal", &[("base", 50.0), ("amount", 3.0)], 50.0);
        assert!((g + 15.0).abs() < 1.5, "cut at 50 Hz was {g} dB");
        let g = gain_db("hum_removal", &[("base", 50.0), ("amount", 3.0)], 150.0);
        assert!((g + 15.0).abs() < 1.5, "cut at the third harmonic was {g} dB");
        let deeper = gain_db("hum_removal", &[("base", 50.0), ("amount", 6.0)], 50.0);
        assert!(deeper < -25.0, "more amount should cut deeper, got {deeper} dB");
        // between harmonics and far away the sound passes
        assert!(gain_db("hum_removal", &[("base", 50.0), ("amount", 3.0)], 75.0).abs() < 0.5);
        assert!(gain_db("hum_removal", &[("base", 50.0), ("amount", 3.0)], 1000.0).abs() < 0.1);
        // 60 Hz mains
        assert!(gain_db("hum_removal", &[("base", 60.0), ("amount", 3.0)], 120.0) < -13.0);
    }

    #[test]
    fn delay_rounds_to_the_nearest_frame() {
        let mut input = vec![[0.0f32; 2]; 4800];
        input[0] = [1.0, 1.0];
        let o = run("delay", &[("delay", 10.7)], input); // 10.7 ms * 48 = 513.6 -> 514 frames
        assert_eq!(o.iter().position(|f| f[0] > 0.5), Some(514));
    }

    #[test]
    fn bass_and_treble_shelves_reach_twelve_db() {
        assert!((gain_db("tone_control", &[("bass", 1.0)], 25.0) - 12.0).abs() < 1.0);
        assert!((gain_db("tone_control", &[("bass", 0.0)], 25.0) + 12.0).abs() < 1.0);
        assert!((gain_db("tone_control", &[("treble", 1.0)], 20000.0) - 12.0).abs() < 1.0);
        // the corners are half way (+6 dB) and the middle of the band is untouched
        assert!((gain_db("tone_control", &[("bass", 1.0)], BASS_CORNER) - 6.0).abs() < 1.0);
        assert!(gain_db("tone_control", &[("bass", 1.0), ("treble", 1.0)], 1000.0).abs() < 1.5);
        assert!(gain_db("tone_control", &[], 1000.0).abs() < 0.05);
    }

    #[test]
    fn channels_modes() {
        let o = run("channels", &[("mode", 1.0)], vec![[0.2, 0.7]; 480]);
        assert_eq!(o[0], [0.7, 0.2]);
        let o = run("channels", &[("mode", 4.0)], vec![[0.2, 0.6]; 480]);
        assert!((o[0][0] - 0.4).abs() < 1e-6 && (o[0][1] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn vu_needle_reads_a_voltage_scale() {
        // full scale at +3 VU (-15 dBFS), and beyond it the needle rests on the stop
        assert!((vu_angle(-15.0) - 45.0).abs() < 1e-3);
        assert_eq!(vu_angle(0.0), 45.0);
        // 0 VU (-18 dBFS) at about 71 % of the arc
        let zero = (vu_angle(-18.0) + 45.0) / 90.0;
        assert!((zero - 0.708).abs() < 0.005, "0 VU at {zero}");
        // -20 VU at about 7 %, silence at the left stop
        let low = (vu_angle(-38.0) + 45.0) / 90.0;
        assert!((low - 0.0708).abs() < 0.005, "-20 VU at {low}");
        assert!((vu_angle(-120.0) + 45.0).abs() < 0.01);
        let mut last = -46.0;
        for i in -1000..=100 { let a = vu_angle(i as f32 / 10.0); assert!(a >= last, "not monotonic at {i}"); last = a; }
    }

    #[test]
    fn ducking_follows_the_overlay() {
        let mut d = Dsp::default();
        let loud = vec![[0.1f32; 2]; 480]; // -20 dB, above the -30 dB threshold
        let bg = vec![[0.5f32; 2]; 480];
        let mut out = vec![];
        for _ in 0..10 { out = d.duck(&loud, &bg, -30.0, 0.4); }
        assert!((out[479][0] - (0.1 + 0.5 * 0.4)).abs() < 1e-4);
        let quiet = vec![[0.0f32; 2]; 480];
        // still ducked inside the 0.5 s hold
        for _ in 0..40 { out = d.duck(&quiet, &bg, -30.0, 0.4); }
        assert!((out[479][0] - 0.2).abs() < 1e-4, "held at {}", out[479][0]);
        for _ in 0..30 { out = d.duck(&quiet, &bg, -30.0, 0.4); }
        assert!((out[479][0] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn compressor_follows_its_static_curve() {
        let set = |a: f64, s: f64, m: f64| [("amount", a), ("speed", s), ("makeup", m)];
        // amount 1: threshold -30 dBFS, 6:1. A 0.5 sine is -9 dBFS RMS, 21 dB over: out = -30 + 21/6 dB RMS
        let o = run("compressor", &set(1.0, 0.0, 1.0), sine(440.0, 0.5, 48000));
        let lat = (COMP_LOOKAHEAD_S * SR) as usize;
        assert!(o[..lat].iter().all(|f| f[0] == 0.0), "look-ahead delay");
        let want = db_to_lin(-30.0 + 21.0 / 6.0) * std::f32::consts::SQRT_2;
        let got = peak_tail(&o);
        assert!((lin_to_db(got) - lin_to_db(want)).abs() < 1.0, "peak {got}, expected about {want}");
        // amount 0 is off: unity
        let o = run("compressor", &set(0.0, 0.0, 1.0), sine(440.0, 0.5, 48000));
        assert!((peak_tail(&o) - 0.5).abs() < 0.005);
        // makeup is a plain linear gain
        let o = run("compressor", &set(0.0, 0.0, 1.5), sine(440.0, 0.2, 48000));
        assert!((peak_tail(&o) - 0.3).abs() < 0.005);
        // quiet audio well under the threshold is left alone
        let o = run("compressor", &set(1.0, 0.0, 1.0), sine(440.0, 0.003, 48000));
        assert!((peak_tail(&o) - 0.003).abs() < 0.0002);
        // more amount means more reduction
        let soft = run("compressor", &set(0.3, 0.0, 1.0), sine(440.0, 0.5, 48000));
        let hard = run("compressor", &set(0.9, 0.0, 1.0), sine(440.0, 0.5, 48000));
        assert!(peak_tail(&hard) < peak_tail(&soft));
    }

    #[test]
    fn compressor_speed_sets_attack() {
        // a step from silence to a loud tone: the fast setting has reduced the gain sooner
        let mut input = vec![[0.0f32; 2]; 4800];
        input.extend(sine(440.0, 0.5, 9600));
        let early = |o: &[Frame]| o[4800 + 480..4800 + 960].iter().fold(0f32, |m, f| m.max(f[0].abs()));
        let slow = run("compressor", &[("amount", 1.0), ("speed", 0.0)], input.clone());
        let fast = run("compressor", &[("amount", 1.0), ("speed", 1.0)], input);
        assert!(early(&fast) < early(&slow) * 0.8, "fast {} slow {}", early(&fast), early(&slow));
        // the static curve is continuous through the knee
        let mut last = 1.0;
        for i in -400..=100 { let g = Compressor::curve(i as f32 / 10.0, -20.0, 4.0); assert!(g <= last + 1e-5); last = g; }
        assert_eq!(Compressor::curve(-40.0, -20.0, 4.0), 0.0);
        assert!((Compressor::curve(0.0, -20.0, 4.0) + 15.0).abs() < 1e-4);
    }

    #[test]
    fn loudness_lifts_bass_and_never_clips() {
        let lb = run("loudness", &[("amount", 0.5)], sine(60.0, 0.05, 48000));
        assert!(lb.iter().all(|f| f[0].is_finite()));
        assert!(peak_tail(&lb) > 0.05 * 1.4, "quiet bass should come out louder, got {}", peak_tail(&lb));
        let hot = run("loudness", &[("amount", 1.0), ("makeup", 4.0)], sine(80.0, 0.95, 48000));
        assert!(hot.iter().all(|f| f[0].abs() <= db_to_lin(-1.0) + 1e-4), "loudness must stay under -1 dBFS");
    }

    #[test]
    fn filters_cut_and_pass_the_right_frequencies() {
        let g = gain_db;
        assert!(g("low_pass", &[("freq", 2000.0)], 12000.0) < -15.0);
        assert!(g("low_pass", &[("freq", 2000.0)], 300.0).abs() < 1.0);
        // Butterworth: -3 dB at the cutoff
        assert!((g("low_pass", &[("freq", 2000.0)], 2000.0) + 3.0).abs() < 0.5);
        assert!(g("high_pass", &[("freq", 500.0)], 40.0) < -15.0);
        assert!(g("high_pass", &[("freq", 500.0)], 5000.0).abs() < 1.0);
        assert!(g("band_pass", &[("freq", 1000.0), ("q", 2.0)], 100.0) < -12.0);
        assert!(g("band_pass", &[("freq", 1000.0), ("q", 2.0)], 1000.0).abs() < 1.0);
        assert!((g("high_shelf", &[("freq", 4000.0), ("gain", 12.0)], 16000.0) - 12.0).abs() < 2.0);
        assert!(g("high_shelf", &[("freq", 4000.0), ("gain", 12.0)], 200.0).abs() < 1.0);
        assert!((g("low_shelf", &[("freq", 500.0), ("gain", -12.0)], 50.0) + 12.0).abs() < 2.0);
        assert!(g("low_shelf", &[("freq", 500.0), ("gain", -12.0)], 8000.0).abs() < 1.0);
    }

    #[test]
    fn parametric_eq_bands_work_and_can_be_switched_off() {
        let boost = |on: f64| run("parametric_eq", &[("bands", 1.0), ("b0_type", 0.0), ("b0_freq", 1000.0), ("b0_gain", 12.0), ("b0_q", 1.0), ("b0_on", on)], sine(1000.0, 0.1, 48000));
        assert!((lin_to_db(peak_tail(&boost(1.0)) / 0.1) - 12.0).abs() < 1.0);
        assert!((peak_tail(&boost(0.0)) - 0.1).abs() < 0.002);
        let notch = run("parametric_eq", &[("bands", 1.0), ("b0_type", 6.0), ("b0_freq", 1000.0), ("b0_q", 4.0), ("b0_on", 1.0)], sine(1000.0, 0.1, 48000));
        assert!(peak_tail(&notch) < 0.01);
        let flat = run("parametric_eq", &[("bands", 0.0)], sine(1000.0, 0.1, 4800));
        assert!((peak_tail(&flat) - 0.1).abs() < 0.002);
    }

    #[test]
    fn click_removal_limits_the_slew_and_leaves_music_alone() {
        let mut input = vec![[0.0f32; 2]; 960];
        input[300] = [0.9, 0.9];
        let o = run("click_removal", &[("sens", 3.0)], input);
        assert!((o[300][0] - 0.5 / 3.0).abs() < 1e-6, "spike was {}", o[300][0]);
        assert!(o[302][0].abs() < 1e-6, "back to silence right after");
        let tone = run("click_removal", &[("sens", 3.0)], sine(1000.0, 0.8, 9600));
        let clean = sine(1000.0, 0.8, 9600);
        assert!(tone.iter().zip(&clean).all(|(a, b)| (a[0] - b[0]).abs() < 1e-6), "a loud 1 kHz tone must pass untouched");
    }

    #[test]
    fn time_shift_pauses_resumes_where_it_left_off_and_jumps_to_live() {
        let mut d = Dsp::default();
        let feed = |d: &mut Dsp, set: &[(&str, f64)], v: f32| -> Vec<Frame> {
            let b = blk("time_shift", set);
            let mut x = vec![[v; 2]; 480];
            d.process(&b, &mut x);
            x
        };
        for _ in 0..5 { feed(&mut d, &[], 0.3); }
        for _ in 0..3 { let o = feed(&mut d, &[("paused", 1.0)], 0.7); assert!(o.iter().all(|f| f[0] == 0.0), "silent while paused"); }
        let o = feed(&mut d, &[("paused", 0.0)], 0.9);
        assert!((o[100][0] - 0.7).abs() < 1e-6, "playback resumes behind live with what was missed, got {}", o[100][0]);
        assert!((d.aux - 1440.0 / 48000.0).abs() < 1e-6);
        let o = feed(&mut d, &[("live_seq", 1.0), ("jump_live", 1.0)], 0.9);
        assert!((o[100][0] - 0.9).abs() < 1e-6, "jump to live");
        assert_eq!(d.aux, 0.0);
        // rewinding replays older audio
        let o = feed(&mut d, &[("live_seq", 1.0), ("jump_seq", 1.0), ("jump_s", 0.02)], 0.1);
        assert!((o[0][0] - 0.9).abs() < 1e-6 || (o[0][0] - 0.7).abs() < 1e-6 || (o[0][0] - 0.3).abs() < 1e-6, "got {}", o[0][0]);
        assert!(d.aux > 0.019);
    }

    #[test]
    fn denoise_removes_learned_noise_but_keeps_new_sound() {
        let mut seed = 12345u32;
        let mut noise = move |n: usize| -> Vec<Frame> {
            (0..n).map(|_| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); let v = ((seed >> 8) as f32 / 8388608.0 - 1.0) * 0.1; [v, v] }).collect()
        };
        let b = blk("noise_reduction", &[("reduction", 12.0), ("learn_seq", 1.0)]);
        let mut d = Dsp::default();
        let run_chunks = |d: &mut Dsp, input: Vec<Frame>| -> Vec<Frame> {
            let mut out = vec![];
            for c in input.chunks(480) { let mut v = c.to_vec(); d.process(&b, &mut v); out.extend(v); }
            out
        };
        run_chunks(&mut d, noise((SR * DENOISE_LEARN_S) as usize + 960));
        assert!((d.aux - DENOISE_LEARN_S as f64).abs() < 0.05, "learned {} s", d.aux);
        let input = noise(48000);
        let out = run_chunks(&mut d, input.clone());
        let ratio = rms(&out[4800..]) / rms(&input[4800..]);
        assert!(ratio < 0.5, "noise should drop by about the reduction, ratio {ratio}");
        // a loud tone on top of the same noise still comes through
        let tone = sine(1000.0, 0.5, 48000);
        let mixed: Vec<Frame> = noise(48000).iter().zip(&tone).map(|(n, t)| [n[0] + t[0], n[1] + t[1]]).collect();
        let out = run_chunks(&mut d, mixed);
        assert!(rms(&out[9600..]) > 0.5 * 0.7071 * 0.8, "tone level {}", rms(&out[9600..]));
    }

    fn write_ir(name: &str, taps: &[f32], rate: u32) -> String {
        let p = std::env::temp_dir().join(format!("kalango-ir-{}-{name}.wav", std::process::id())).to_string_lossy().to_string();
        let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(&p, spec).unwrap();
        for t in taps { w.write_sample(*t).unwrap(); }
        w.finalize().unwrap();
        p
    }

    fn run_text(kind: &str, nset: &[(&str, f64)], sset: &[(&str, &str)], input: Vec<Frame>) -> (Vec<Frame>, Dsp) {
        let mut b = blk(kind, nset);
        for (k, v) in sset { b.p.set_text(k, v); }
        let mut d = Dsp::default();
        let mut out = vec![];
        for c in input.chunks(480) { let mut v = c.to_vec(); d.process(&b, &mut v); out.extend(v); }
        (out, d)
    }

    #[test]
    fn fir_filter_convolves_with_the_impulse_response() {
        // a delayed spike delays the sound by exactly that many frames
        let mut ir = vec![0.0f32; 101];
        ir[100] = 1.0;
        let path = write_ir("delay", &ir, 48000);
        let (o, d) = run_text("convolution", &[], &[("ir", &path)], sine(440.0, 0.5, 9600));
        let input = sine(440.0, 0.5, 9600);
        for i in 200..9000 { assert!((o[i][0] - input[i - 100][0]).abs() < 1e-3, "frame {i}"); }
        assert!(d.note.contains("101 taps"), "{}", d.note);
        // a half-size spike plus 6 dB of input gain gives the same level back
        let path = write_ir("half", &[0.5], 48000);
        let (o, _) = run_text("convolution", &[("pregain", 6.0206)], &[("ir", &path)], sine(440.0, 0.4, 4800));
        assert!((peak_tail(&o) - 0.4).abs() < 0.01);
        // a 24 kHz impulse is resampled and says so
        let path = write_ir("slow", &vec![0.1f32; 400], 24000);
        let (_, d) = run_text("convolution", &[], &[("ir", &path)], sine(440.0, 0.4, 960));
        assert!(d.note.contains("resampled"), "{}", d.note);
        // a missing file is reported, and the sound still passes
        let (o, d) = run_text("convolution", &[], &[("ir", "/not/here.wav")], sine(440.0, 0.4, 4800));
        assert!(d.note.starts_with("error"));
        assert!((peak_tail(&o) - 0.4).abs() < 0.01);
        // no impulse chosen: plain pass-through
        let (o, _) = run_text("convolution", &[], &[], sine(440.0, 0.4, 4800));
        assert!((peak_tail(&o) - 0.4).abs() < 0.01);
    }

    #[test]
    fn speech_denoise_lowers_steady_noise() {
        let mut seed = 99u32;
        let noise: Vec<Frame> = (0..48000 * 2).map(|_| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); let v = ((seed >> 8) as f32 / 8388608.0 - 1.0) * 0.05; [v, v] }).collect();
        for mono in [0.0, 1.0] {
            let (o, d) = run_text("voice_denoise", &[("mono", mono)], &[], noise.clone());
            assert!(o.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
            let ratio = rms(&o[48000..]) / rms(&noise[48000..]);
            assert!(ratio < 0.5, "mono={mono}: steady noise should drop a lot, ratio {ratio}");
            assert!((0.0..=1.0).contains(&d.aux));
        }
    }

    #[test]
    fn mixer_applies_gain_balance_pan_solo_and_fades() {
        let one = |v: f32| vec![[v, v]; 480];
        let mut fade = [0.0f32; 5];
        let p = |set: &[(&str, f64)]| { let mut b = blk("mixer", &[]); for (k, v) in set { b.p.set(k, *v); } b.p };
        // gain and mute
        let o = mix_tracks(&[one(0.4), one(0.2)], &p(&[("g1", 0.5), ("m0", 0.0)]), &mut fade);
        assert!((o[0][0] - 0.5).abs() < 1e-6);
        let o = mix_tracks(&[one(0.4), one(0.2)], &p(&[("m0", 1.0)]), &mut fade);
        assert!((o[0][0] - 0.2).abs() < 1e-6);
        // solo keeps only the soloed track
        let o = mix_tracks(&[one(0.4), one(0.2)], &p(&[("s1", 1.0)]), &mut fade);
        assert!((o[0][0] - 0.2).abs() < 1e-6);
        // balance and pan use the same laws as the Balance and Pan blocks
        let o = mix_tracks(&[one(1.0)], &p(&[("b0", 0.5)]), &mut fade);
        assert!((o[0][0] - 0.5).abs() < 1e-6 && (o[0][1] - 1.0).abs() < 1e-6);
        let o = mix_tracks(&[vec![[1.0, 0.0]; 480]], &p(&[("p0", 0.5)]), &mut fade);
        assert!((o[0][0] - 0.70710677).abs() < 1e-5 && (o[0][1] - 0.70710677).abs() < 1e-5);
        // a fade moves a little each chunk and reaches silence after a second
        let mut fade = [0.0f32; 5];
        let fo = p(&[("fo0", 1.0)]);
        let first = mix_tracks(&[one(1.0)], &fo, &mut fade);
        assert!(first[0][0] < 1.0 && first[0][0] > 0.9);
        for _ in 0..120 { mix_tracks(&[one(1.0)], &fo, &mut fade); }
        assert_eq!(mix_tracks(&[one(1.0)], &fo, &mut fade)[0][0], 0.0);
        let back = p(&[("fo0", 0.0)]);
        for _ in 0..120 { mix_tracks(&[one(1.0)], &back, &mut fade); }
        assert!((mix_tracks(&[one(1.0)], &back, &mut fade)[0][0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn widener_spreads_mono_keeps_bass_centred_and_folds_back_to_mono() {
        let mono = sine(3000.0, 0.4, 48000);
        for xo in [500.0, 50.0] {
            let o = run("stereo_widen", &[("crossover", xo)], mono.clone());
            assert!(o.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
            let diff = o[24000..].iter().map(|f| (f[0] - f[1]).abs()).fold(0.0, f32::max);
            assert!(diff > 0.1, "crossover {xo}: left and right should differ for mono input, diff {diff}");
            // the mono fold-down (L + R) / 2 is the input scaled by a constant
            let k = 1.0 / (1.0 + WIDEN_WIDTH * WIDEN_WIDTH).sqrt();
            assert!(o.iter().zip(&mono).all(|(a, b)| (0.5 * (a[0] + a[1]) - b[0] * k).abs() < 1e-5), "crossover {xo}: not mono-compatible");
        }
        // real stereo passes through untouched
        let st: Vec<Frame> = (0..4800).map(|i| [(i as f32 * 0.01).sin() * 0.3, (i as f32 * 0.013).sin() * 0.3]).collect();
        let o = run("stereo_widen", &[("crossover", 500.0)], st.clone());
        assert_eq!(o, st);
        // below the crossover the sound stays in the middle
        let side = |f: f32| run("stereo_widen", &[("crossover", 500.0)], sine(f, 0.4, 48000))[24000..].iter().map(|f| (f[0] - f[1]).abs()).fold(0.0, f32::max);
        let (d_low, d_high) = (side(60.0), side(3000.0));
        assert!(d_low < 0.01 && d_high > 0.3, "low band side {d_low}, high band side {d_high}");
    }
}
