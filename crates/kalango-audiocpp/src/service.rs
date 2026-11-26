//! One `audiocpp_server` per program, started on first use and looked after: the model stays loaded while somebody
//! uses it, the server stops by itself a while after the last user left, a crashed server is started again with a
//! growing wait, and the child never outlives the program.
//!
//! A single manager thread owns the [`Server`] for its whole life. That matters: `PR_SET_PDEATHSIG` fires when the
//! *thread* that spawned the child ends, so the child must be spawned by a thread that lives as long as the service.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{Backend, Config, Error, Result, Server, Stream, SAMPLE_RATE};

/// How a [`Service`] behaves.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    /// How the server is started (program, model, backend…).
    pub server: Config,
    /// The server is stopped this long after the last [`Lease`] ended (its memory and video memory go back).
    pub idle_timeout: Duration,
    /// Wait before the first retry; it doubles on every failure up to `backoff_max`.
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    /// This many failures in a row and the service gives up until [`Service::retry`].
    pub max_attempts: u32,
    /// Time one second of sound through the new server to know how fast it is.
    pub probe: bool,
    /// A server whose measured speed (processing time / sound time) is at or above this cannot do live sound.
    pub live_rtf_limit: f32,
    /// A run that lasted this long counts as healthy: the failures before it are forgotten.
    pub healthy_after: Duration,
}

impl ServiceConfig {
    pub fn new(server: Config) -> Self {
        Self {
            server,
            idle_timeout: Duration::from_secs(120),
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            max_attempts: 5,
            probe: true,
            live_rtf_limit: 0.8,
            healthy_after: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Nothing is running (never used yet, or stopped after the idle time).
    Idle,
    Starting,
    /// `rtf` is the measured processing time over sound time of one second of sound (`None` when not measured).
    Running {
        backend: Backend,
        rtf: Option<f32>,
    },
    /// Failed; the next try is at `retry_at`.
    Backoff {
        attempt: u32,
        reason: String,
        retry_at: Instant,
    },
    /// Gave up after too many failures in a row.
    Unavailable {
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: State,
    /// Servers started so far.
    pub starts: u32,
    /// Process id of the running server.
    pub pid: Option<u32>,
    /// Limit copied from the configuration, so [`Status::live_ok`] needs nothing else.
    live_rtf_limit: f32,
}

impl Status {
    /// `Ok` when the running server can keep up with live sound; otherwise why not.
    pub fn live_ok(&self) -> std::result::Result<(), LiveLimit> {
        match &self.state {
            State::Running { backend, rtf } => {
                if !backend.is_gpu() {
                    return Err(LiveLimit::CpuOnly { rtf: *rtf });
                }
                match rtf {
                    Some(r) if *r >= self.live_rtf_limit => Err(LiveLimit::TooSlow { rtf: *r }),
                    _ => Ok(()),
                }
            }
            _ => Err(LiveLimit::NotRunning),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LiveLimit {
    NotRunning,
    /// Only the CPU is used: the model runs several times slower than real time.
    CpuOnly {
        rtf: Option<f32>,
    },
    TooSlow {
        rtf: f32,
    },
}

enum Cmd {
    Wake,
    Retry,
    OpenStream(Sender<Result<Stream>>),
    Shutdown(Sender<()>),
}

struct Shared {
    status: Mutex<Status>,
    users: AtomicUsize,
    closed: AtomicBool,
}

pub struct Service {
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// While one exists, the server is wanted. Dropping it lets the idle timer run.
pub struct Lease {
    service: Arc<Service>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.service.shared.users.fetch_sub(1, Ordering::SeqCst);
        let _ = self.service.tx.send(Cmd::Wake);
    }
}

impl Service {
    /// Creates the service. Nothing is started until the first [`Service::lease`].
    pub fn new(config: ServiceConfig) -> Arc<Service> {
        let status = Status { state: State::Idle, starts: 0, pid: None, live_rtf_limit: config.live_rtf_limit };
        let shared =
            Arc::new(Shared { status: Mutex::new(status), users: AtomicUsize::new(0), closed: AtomicBool::new(false) });
        let (tx, rx) = mpsc::channel();
        let sh = shared.clone();
        let handle = std::thread::Builder::new()
            .name("audiocpp-manager".into())
            .spawn(move || manage(rx, sh, config))
            .expect("manager thread");
        Arc::new(Service { shared, tx, handle: Mutex::new(Some(handle)) })
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// Says that the server is wanted (and starts it when it is not running).
    pub fn lease(self: &Arc<Self>) -> Lease {
        self.shared.users.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Wake);
        Lease { service: self.clone() }
    }

    /// After giving up ([`State::Unavailable`]) or during a wait: try again now.
    pub fn retry(&self) {
        let _ = self.tx.send(Cmd::Retry);
    }

    /// A live stream on the running server (an error when it is not running; check [`Service::status`] or use
    /// [`Service::wait_running`] first).
    pub fn open_stream(&self) -> Result<Stream> {
        let (reply, answer) = mpsc::channel();
        self.tx.send(Cmd::OpenStream(reply)).map_err(|_| Error::Protocol("the service was shut down".into()))?;
        answer.recv_timeout(Duration::from_secs(15)).map_err(|_| Error::Timeout("the manager did not answer"))?
    }

    /// Waits until the server runs. Gives up when it becomes unavailable, after `timeout`, or when `cancel` is set.
    pub fn wait_running(&self, timeout: Duration, cancel: &AtomicBool) -> Result<Status> {
        let deadline = Instant::now() + timeout;
        loop {
            let st = self.status();
            match &st.state {
                State::Running { .. } => return Ok(st),
                State::Unavailable { reason } => return Err(Error::Protocol(reason.clone())),
                _ => {}
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Protocol("cancelled".into()));
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout("the server did not become ready"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Stops the server and the manager, waiting up to `wait` for them. Safe to call twice. (Whatever happens, the
    /// child dies with the program: `PR_SET_PDEATHSIG`.)
    pub fn shutdown(&self, wait: Duration) {
        if self.shared.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let (ack_tx, ack) = mpsc::channel();
        if self.tx.send(Cmd::Shutdown(ack_tx)).is_ok() {
            let _ = ack.recv_timeout(wait);
        }
        if let Some(h) = self.handle.lock().unwrap().take() {
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(5));
    }
}

fn set_state(shared: &Shared, state: State, pid: Option<u32>, started: bool) {
    let mut st = shared.status.lock().unwrap();
    st.state = state;
    st.pid = pid;
    if started {
        st.starts += 1;
    }
}

/// The last lines of the server's log: why it stopped.
fn log_tail(path: &std::path::Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    tail.join(" | ")
}

/// Time one second of noise through a fresh stream: processing seconds per second of sound.
fn probe(server: &Server) -> Result<f32> {
    let mut stream = server.open_stream()?;
    let mut state = 7u32;
    let noise: Vec<f32> = (0..SAMPLE_RATE as usize)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            0.05 * ((state >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
        })
        .collect();
    let t = Instant::now();
    for block in noise.chunks(960) {
        stream.push(block)?;
    }
    let out = stream.finish()?;
    if out.is_empty() {
        return Err(Error::Protocol("the server returned no sound for the probe".into()));
    }
    Ok(t.elapsed().as_secs_f32())
}

fn start_server(cfg: &ServiceConfig) -> std::result::Result<(Server, Backend, Option<f32>), String> {
    let log = cfg.server.work_dir.join("audiocpp-server.log");
    // keep the previous run's log for the person who wants to know why it stopped
    let _ = std::fs::rename(&log, cfg.server.work_dir.join("audiocpp-server.log.1"));
    let server = Server::spawn(&cfg.server).map_err(|e| match e {
        Error::ServerExited(code) => format!("the server stopped with code {code}: {}", log_tail(&log)),
        Error::Timeout(_) => format!("the server did not answer in time: {}", log_tail(&log)),
        other => other.to_string(),
    })?;
    let backend = server.reported_backend().ok().flatten().unwrap_or(cfg.server.backend);
    let rtf = if cfg.probe { Some(probe(&server).map_err(|e| format!("the speed test failed: {e}"))?) } else { None };
    Ok((server, backend, rtf))
}

fn manage(rx: mpsc::Receiver<Cmd>, shared: Arc<Shared>, cfg: ServiceConfig) {
    let mut server: Option<Server> = None;
    let mut failures = 0u32;
    let mut next_try = Instant::now();
    let mut healthy_since = Instant::now();
    let mut unused_since = Some(Instant::now());
    let mut gave_up = false;

    let stop = |server: &mut Option<Server>| {
        if let Some(s) = server.take() {
            let _ = s.stop(Duration::from_secs(2));
        }
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Cmd::Shutdown(ack)) => {
                stop(&mut server);
                set_state(&shared, State::Idle, None, false);
                let _ = std::fs::remove_dir_all(&cfg.server.work_dir);
                let _ = ack.send(());
                return;
            }
            Ok(Cmd::OpenStream(reply)) => {
                let r = match server.as_mut() {
                    Some(s) => {
                        if s.is_running() {
                            s.open_stream()
                        } else {
                            Err(Error::Protocol("the voice isolation server is not running".into()))
                        }
                    }
                    None => Err(Error::Protocol("the voice isolation server is not running".into())),
                };
                let _ = reply.send(r);
            }
            Ok(Cmd::Retry) => {
                failures = 0;
                gave_up = false;
                next_try = Instant::now();
            }
            Ok(Cmd::Wake) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                stop(&mut server);
                return;
            }
        }

        let wanted = shared.users.load(Ordering::SeqCst) > 0;

        // a dead server
        if let Some(s) = server.as_mut() {
            if !s.is_running() {
                let tail = log_tail(s.log_path());
                server = None;
                failures += 1;
                let reason = format!("the server stopped unexpectedly: {tail}");
                if failures >= cfg.max_attempts {
                    gave_up = true;
                    set_state(&shared, State::Unavailable { reason }, None, false);
                } else {
                    next_try = Instant::now() + backoff(&cfg, failures);
                    set_state(&shared, State::Backoff { attempt: failures, reason, retry_at: next_try }, None, false);
                }
            } else if failures > 0 && healthy_since.elapsed() >= cfg.healthy_after {
                failures = 0;
            }
        }

        // idle: nobody uses it
        if wanted {
            unused_since = None;
        } else if unused_since.is_none() {
            unused_since = Some(Instant::now());
        }
        if server.is_some() && unused_since.is_some_and(|t| t.elapsed() >= cfg.idle_timeout) {
            stop(&mut server);
            set_state(&shared, State::Idle, None, false);
        }
        if !wanted && !gave_up && !matches!(shared.status.lock().unwrap().state, State::Idle) && server.is_none() {
            // nobody waits for the retry any more
            set_state(&shared, State::Idle, None, false);
        }

        // start (lazily, on first use)
        if wanted && server.is_none() && !gave_up && Instant::now() >= next_try {
            set_state(&shared, State::Starting, None, false);
            match start_server(&cfg) {
                Ok((s, backend, rtf)) => {
                    healthy_since = Instant::now();
                    set_state(&shared, State::Running { backend, rtf }, Some(s.pid()), true);
                    server = Some(s);
                }
                Err(reason) => {
                    failures += 1;
                    if failures >= cfg.max_attempts {
                        gave_up = true;
                        set_state(&shared, State::Unavailable { reason }, None, false);
                    } else {
                        next_try = Instant::now() + backoff(&cfg, failures);
                        set_state(
                            &shared,
                            State::Backoff { attempt: failures, reason, retry_at: next_try },
                            None,
                            false,
                        );
                    }
                }
            }
        }
    }
}

fn backoff(cfg: &ServiceConfig, failures: u32) -> Duration {
    let factor = 1u32 << failures.saturating_sub(1).min(16);
    (cfg.backoff_base * factor).min(cfg.backoff_max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_stops_at_the_limit() {
        let cfg = ServiceConfig::new(Config::new("/x".into(), "/m".into(), "/w".into()));
        let waits: Vec<u64> = (1..=7).map(|n| backoff(&cfg, n).as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 30, 30]);
    }

    #[test]
    fn live_needs_a_fast_gpu_server() {
        let mk = |state| Status { state, starts: 1, pid: Some(1), live_rtf_limit: 0.8 };
        assert_eq!(mk(State::Running { backend: Backend::Cuda, rtf: Some(0.2) }).live_ok(), Ok(()));
        assert_eq!(mk(State::Running { backend: Backend::Cuda, rtf: None }).live_ok(), Ok(()));
        assert_eq!(
            mk(State::Running { backend: Backend::Cuda, rtf: Some(1.4) }).live_ok(),
            Err(LiveLimit::TooSlow { rtf: 1.4 })
        );
        assert_eq!(
            mk(State::Running { backend: Backend::Cpu, rtf: Some(3.2) }).live_ok(),
            Err(LiveLimit::CpuOnly { rtf: Some(3.2) })
        );
        assert_eq!(mk(State::Idle).live_ok(), Err(LiveLimit::NotRunning));
    }
}
