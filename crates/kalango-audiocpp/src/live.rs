//! Voice isolation inside a real-time audio loop.
//!
//! [`LiveIsolator::process`] is called from the audio thread, once per small chunk of stereo sound. It never waits for
//! the model: the mono mix of each chunk goes to a worker thread that talks to the server, the isolated sound comes
//! back into a queue, and `process` mixes it with the original **by sample position**. Because the model answers about
//! 80 to 100 ms late, the original is held for the same time (that is the latency: the sound that comes out is the
//! sound that went in that long ago), so the wet and dry parts always line up instead of echoing. While the model is
//! not ready (starting, restarting, too slow) the original passes through untouched.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::service::{Lease, LiveLimit, Service, State};
use crate::SAMPLE_RATE;

/// The isolated sound waits this long in the queue before the first sample goes out (jitter margin), in samples.
const PRIME: usize = 960;
/// More than this much sound waiting for the model means it cannot keep up (samples, 0.6 s).
const MAX_LAG: usize = SAMPLE_RATE as usize * 6 / 10;
/// What goes to the server in one push: 20 ms (the model is twice as fast on 20 ms blocks as on 10 ms ones).
const PUSH: usize = 960;

/// What the block shows.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveState {
    /// The server or the stream is being started; the sound passes through unchanged.
    Starting,
    /// The sound goes through the model.
    Running,
    /// Temporarily not isolating (the server restarts, the model fell behind…); the sound passes through unchanged.
    Fallback(String),
    /// Cannot work (gave up after repeated failures); the sound passes through unchanged.
    Unavailable(String),
    /// The server runs, but too slowly for live sound (CPU only): use the offline action instead.
    OfflineOnly(LiveLimit),
}

struct Shared {
    state: Mutex<LiveState>,
    /// Generation of the running stream; 0 while there is none. The audio thread starts from scratch when it changes.
    generation: AtomicU64,
    iso: Mutex<VecDeque<f32>>,
    stop: AtomicBool,
    /// Sound held for alignment, in samples (the latency).
    lag: AtomicU32,
    /// Times the output ran out of isolated sound and went silent for a moment.
    gaps: AtomicU32,
}

impl Shared {
    fn set_state(&self, s: LiveState) {
        let mut cur = self.state.lock().unwrap();
        if *cur != s {
            *cur = s;
        }
    }

    /// The stream stopped being usable: the audio thread passes the original through.
    fn fail(&self, why: &str) {
        self.generation.store(0, Ordering::Release);
        self.set_state(LiveState::Fallback(why.to_string()));
    }
}

pub struct LiveIsolator {
    shared: Arc<Shared>,
    tx: SyncSender<Vec<f32>>,
    handle: Option<std::thread::JoinHandle<()>>,
    // audio-thread state
    generation: u64,
    dry: VecDeque<[f32; 2]>,
    primed: bool,
}

impl LiveIsolator {
    /// Wants the server (starting it on the first use) and keeps a stream on it for as long as this exists.
    pub fn new(service: Arc<Service>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(LiveState::Starting),
            generation: AtomicU64::new(0),
            iso: Mutex::new(VecDeque::new()),
            stop: AtomicBool::new(false),
            lag: AtomicU32::new(0),
            gaps: AtomicU32::new(0),
        });
        let (tx, rx) = mpsc::sync_channel(256);
        let lease = service.lease();
        let sh = shared.clone();
        let handle = std::thread::Builder::new()
            .name("live-isolator".into())
            .spawn(move || worker(service, lease, sh, rx))
            .expect("worker thread");
        Self { shared, tx, handle: Some(handle), generation: 0, dry: VecDeque::new(), primed: false }
    }

    pub fn state(&self) -> LiveState {
        self.shared.state.lock().unwrap().clone()
    }

    /// How late the isolated sound comes out, in milliseconds (0 while the sound passes through).
    pub fn latency_ms(&self) -> f32 {
        self.shared.lag.load(Ordering::Relaxed) as f32 * 1000.0 / SAMPLE_RATE as f32
    }

    /// Moments the output went silent because the model was late.
    pub fn gaps(&self) -> u32 {
        self.shared.gaps.load(Ordering::Relaxed)
    }

    /// Replaces `frames` with the isolated voice mixed with the original: `amount` 1 is only the isolated voice, 0 only
    /// the original (delayed by the latency, so that it lines up).
    pub fn process(&mut self, frames: &mut [[f32; 2]], amount: f32) {
        let generation = self.shared.generation.load(Ordering::Acquire);
        if generation == 0 {
            if self.generation != 0 {
                self.generation = 0;
                self.dry.clear();
                self.primed = false;
                self.shared.lag.store(0, Ordering::Relaxed);
            }
            return;
        }
        if generation != self.generation {
            self.generation = generation;
            self.dry.clear();
            self.primed = false;
        }
        let amount = amount.clamp(0.0, 1.0);
        let mono: Vec<f32> = frames.iter().map(|f| 0.5 * (f[0] + f[1])).collect();
        match self.tx.try_send(mono) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.fail("the voice model is not keeping up");
                return;
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.fail("the voice isolation worker stopped");
                return;
            }
        }
        self.dry.extend(frames.iter().copied());
        let mut iso = self.shared.iso.lock().unwrap();
        if !self.primed && iso.len() >= PRIME {
            self.primed = true;
        }
        let ready = if self.primed { frames.len().min(iso.len()) } else { 0 };
        for (i, out) in frames.iter_mut().enumerate() {
            if i < ready {
                let wet = iso.pop_front().unwrap_or(0.0);
                let dry = self.dry.pop_front().unwrap_or([0.0; 2]);
                *out = [amount * wet + (1.0 - amount) * dry[0], amount * wet + (1.0 - amount) * dry[1]];
            } else {
                *out = [0.0; 2];
            }
        }
        drop(iso);
        if self.primed && ready < frames.len() {
            self.shared.gaps.fetch_add(1, Ordering::Relaxed);
        }
        self.shared.lag.store(self.dry.len() as u32, Ordering::Relaxed);
        if self.dry.len() > MAX_LAG {
            self.shared.fail("the voice model is not keeping up");
        }
    }
}

impl Drop for LiveIsolator {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        // the worker ends within a few tens of milliseconds, but a stalled server must not stall the audio engine
        if let Some(h) = self.handle.take() {
            let deadline = Instant::now() + Duration::from_millis(300);
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }
}

fn pause(shared: &Shared, ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until && !shared.stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn worker(service: Arc<Service>, _lease: Lease, shared: Arc<Shared>, rx: mpsc::Receiver<Vec<f32>>) {
    let mut generation = 0u64;
    while !shared.stop.load(Ordering::Acquire) {
        let status = service.status();
        match &status.state {
            State::Running { .. } => {}
            State::Unavailable { reason } => {
                shared.set_state(LiveState::Unavailable(reason.clone()));
                pause(&shared, 200);
                continue;
            }
            State::Backoff { reason, .. } => {
                shared.set_state(LiveState::Fallback(reason.clone()));
                pause(&shared, 100);
                continue;
            }
            State::Idle | State::Starting => {
                shared.set_state(LiveState::Starting);
                pause(&shared, 50);
                continue;
            }
        }
        if let Err(limit) = status.live_ok() {
            shared.set_state(LiveState::OfflineOnly(limit));
            pause(&shared, 300);
            continue;
        }
        let mut stream = match service.open_stream() {
            Ok(s) => s,
            Err(e) => {
                shared.set_state(LiveState::Fallback(e.to_string()));
                pause(&shared, 500);
                continue;
            }
        };
        // a fresh stream: nothing from an older one may be left in either queue
        while rx.try_recv().is_ok() {}
        shared.iso.lock().unwrap().clear();
        generation += 1;
        shared.generation.store(generation, Ordering::Release);
        shared.set_state(LiveState::Running);

        let reason = pump(&mut stream, &shared, &rx);
        shared.generation.store(0, Ordering::Release);
        if let Some(why) = reason {
            shared.set_state(LiveState::Fallback(why));
            pause(&shared, 500);
        }
    }
}

/// Moves sound between the audio thread and the stream until something breaks (`Some(why)`) or the isolator is
/// dropped (`None`).
fn pump(stream: &mut crate::Stream, shared: &Shared, rx: &mpsc::Receiver<Vec<f32>>) -> Option<String> {
    let mut pending: Vec<f32> = Vec::new();
    let mut since = Instant::now();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return None;
        }
        if shared.generation.load(Ordering::Acquire) == 0 {
            // the audio thread gave up on this stream
            return Some("the voice model is not keeping up".into());
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(mut chunk) => {
                if pending.is_empty() {
                    since = Instant::now();
                }
                pending.append(&mut chunk);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
        if pending.len() >= PUSH || (!pending.is_empty() && since.elapsed() >= Duration::from_millis(25)) {
            if let Err(e) = stream.push(&pending) {
                return Some(e.to_string());
            }
            pending.clear();
        }
        match stream.pull(Duration::from_millis(0)) {
            Ok(Some(samples)) => {
                if !samples.is_empty() {
                    shared.iso.lock().unwrap().extend(samples);
                }
            }
            Ok(None) => return Some("the server closed the stream".into()),
            Err(e) => return Some(e.to_string()),
        }
    }
}
