//! Controls an external `audiocpp_server` (audio.cpp, ggml) process, the way the project drives llama.cpp: the model
//! runs in its own process, this crate spawns it, checks it, streams PCM through it and always reaps it.
//!
//! Process rules: absolute binary path, no shell, cleared environment, `PR_SET_PDEATHSIG`, pidfd-based signals and
//! kill-on-drop. Streaming uses the server's `POST /v1/audio/speech/live` (chunked raw PCM in, chunked raw PCM out).

mod http;
mod ownership;
mod process;

use std::fmt;
use std::io::{self, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use process::Process;

/// Model id registered in the generated server configuration.
pub const MODEL_ID: &str = "isolator";
/// The Voice Isolator works at 48 kHz mono.
pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The server process ended before it became healthy (exit code; the log file has the reason).
    ServerExited(i32),
    Timeout(&'static str),
    /// The server answered with an HTTP error (status, body).
    Http(u16, String),
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::ServerExited(c) => write!(f, "audiocpp_server exited with code {c}"),
            Error::Timeout(what) => write!(f, "timeout: {what}"),
            Error::Http(s, b) => write!(f, "http {s}: {b}"),
            Error::Protocol(m) => write!(f, "protocol: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Cuda,
    Vulkan,
    Cpu,
}

impl Backend {
    fn as_str(self) -> &'static str {
        match self {
            Backend::Cuda => "cuda",
            Backend::Vulkan => "vulkan",
            Backend::Cpu => "cpu",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Absolute path of `audiocpp_server`.
    pub server_bin: PathBuf,
    /// A realtime checkpoint: the `.gguf` file or the `denoiser_realtime.safetensors` file (or its directory).
    pub model: PathBuf,
    pub backend: Backend,
    pub threads: u32,
    /// Look-ahead in 10 ms frames: 0, 2 or 4 (latency 42.7, 62.7 or 82.7 ms). `None` keeps the checkpoint default (2).
    pub lookahead_frames: Option<u32>,
    /// Directory for the generated configuration and the server log (created if missing).
    pub work_dir: PathBuf,
    /// Extra environment for the child (the rest is cleared), e.g. `LD_LIBRARY_PATH` for a private CUDA runtime.
    pub env: Vec<(String, String)>,
    /// How long `Server::spawn` waits for `/health`.
    pub startup_timeout: Duration,
    /// Socket timeout for every read and write (an idle stream longer than this is an error).
    pub io_timeout: Duration,
}

impl Config {
    /// A fresh private directory name under `XDG_RUNTIME_DIR` (per user, mode 0700), else the temp dir.
    pub fn private_work_dir() -> PathBuf {
        let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join(format!("kalango-audiocpp-{}", std::process::id()))
    }

    pub fn new(server_bin: PathBuf, model: PathBuf, work_dir: PathBuf) -> Self {
        Self {
            server_bin,
            model,
            backend: Backend::Cuda,
            threads: 1,
            lookahead_frames: None,
            work_dir,
            env: Vec::new(),
            startup_timeout: Duration::from_secs(60),
            io_timeout: Duration::from_secs(10),
        }
    }

    fn server_json(&self, port: u16) -> String {
        let session = match self.lookahead_frames {
            Some(n) => format!(r#","session_options":{{"voice_isolator.lookahead_frames":{n}}}"#),
            None => String::new(),
        };
        format!(
            r#"{{"host":"127.0.0.1","port":{port},"backend":"{}","device":0,"threads":{},"lazy_load":false,"models":[{{"id":"{MODEL_ID}","family":"voice_isolator","path":{},"task":"s2s","mode":"streaming"{session}}}]}}"#,
            self.backend.as_str(),
            self.threads,
            json_string(&self.model.to_string_lossy()),
        )
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A running `audiocpp_server`. Dropping it kills and reaps the process.
pub struct Server {
    process: Process,
    addr: SocketAddr,
    io_timeout: Duration,
    log: PathBuf,
}

impl Server {
    /// Starts the server and waits until it listens and `/health` answers 200 (or the timeout / an early exit).
    ///
    /// The server binds port 0 and the port is read back from the kernel as the child's own listening socket, so no
    /// other process can be mistaken for it (see `ownership`). The work directory must be private (0700, ours).
    pub fn spawn(config: &Config) -> Result<Self> {
        ownership::private_dir(&config.work_dir)?;
        let config_path = ownership::write_private(&config.work_dir, "audiocpp-server.json", &config.server_json(0))?;
        let log = config.work_dir.join("audiocpp-server.log");
        let args = vec!["--config".to_string(), config_path.to_string_lossy().into_owned()];
        let process = Process::spawn(&config.server_bin, &args, &config.env, &log)?;
        let mut server =
            Server { process, addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), io_timeout: config.io_timeout, log };
        let deadline = Instant::now() + config.startup_timeout;
        loop {
            if let Some(code) = server.process.try_wait()? {
                return Err(Error::ServerExited(code));
            }
            if server.addr.port() == 0 {
                if let Ok(Some(port)) = ownership::listening_port_of(server.process.id()) {
                    server.addr.set_port(port);
                }
            }
            if server.addr.port() != 0 && server.health().is_ok() {
                return Ok(server);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout("the server did not become healthy"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Fails unless the port we talk to is still the child's own listening socket.
    fn verify_owner(&self) -> Result<()> {
        match ownership::listening_port_of(self.process.id()) {
            Ok(Some(port)) if port == self.addr.port() => Ok(()),
            _ => Err(Error::Protocol("the port is not the spawned server's listening socket".into())),
        }
    }

    pub fn pid(&self) -> u32 {
        self.process.id()
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn log_path(&self) -> &Path {
        &self.log
    }

    /// `GET /health`; `Ok` only for a 200.
    pub fn health(&self) -> Result<()> {
        self.verify_owner()?;
        let (status, body) = http::get(self.addr, "/health", Duration::from_secs(2))?;
        if status == 200 {
            Ok(())
        } else {
            Err(Error::Http(status, body))
        }
    }

    /// `true` while the process runs.
    pub fn is_running(&mut self) -> bool {
        matches!(self.process.try_wait(), Ok(None))
    }

    /// SIGTERM, then SIGKILL after `grace`; returns the exit code.
    pub fn stop(mut self, grace: Duration) -> Result<i32> {
        Ok(self.process.stop(grace)?)
    }

    /// Opens a live stream: push 48 kHz mono f32 samples, pull isolated samples (latency about 62.7 ms by default).
    pub fn open_stream(&self) -> Result<Stream> {
        self.verify_owner()?;
        Stream::open(self.addr, self.io_timeout)
    }
}

/// One live exchange: chunked PCM (f32 little endian, 48 kHz mono) goes in, 16-bit PCM comes back as `f32`.
pub struct Stream {
    writer: TcpStream,
    rx: Receiver<io::Result<Vec<f32>>>,
    reader: Option<JoinHandle<()>>,
    timeout: Duration,
    finished: bool,
}

impl Stream {
    fn open(addr: SocketAddr, timeout: Duration) -> Result<Self> {
        let mut writer = http::connect(addr, timeout)?;
        write!(
            writer,
            "POST /v1/audio/speech/live?model={MODEL_ID}&sample_rate={SAMPLE_RATE}&channels=1&sample_format=f32le&stream_format=audio HTTP/1.1\r\n\
             Host: {addr}\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )?;
        let read_half = writer.try_clone()?;
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || read_output(read_half, tx));
        Ok(Self { writer, rx, reader: Some(reader), timeout, finished: false })
    }

    /// Sends samples (any length; the model buffers internally).
    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        Ok(http::write_chunk(&mut self.writer, &bytes)?)
    }

    /// Returns whatever isolated audio is ready, waiting up to `wait` for the first piece. `Ok(None)` means the
    /// server closed the stream (after `finish`).
    pub fn pull(&mut self, wait: Duration) -> Result<Option<Vec<f32>>> {
        match self.rx.recv_timeout(wait) {
            Ok(Ok(samples)) => Ok(Some(samples)),
            Ok(Err(e)) => Err(map_reader_error(e)),
            Err(RecvTimeoutError::Timeout) => Ok(Some(Vec::new())),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    /// Ends the input (terminating chunk) and collects the rest of the output.
    pub fn finish(&mut self) -> Result<Vec<f32>> {
        if !self.finished {
            self.finished = true;
            self.writer.write_all(b"0\r\n\r\n")?;
        }
        let mut all = Vec::new();
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.rx.recv_timeout(self.timeout) {
                Ok(Ok(mut part)) => {
                    all.append(&mut part);
                    if Instant::now() >= deadline + self.timeout * 6 {
                        return Err(Error::Timeout("the output did not end"));
                    }
                }
                Ok(Err(e)) => return Err(map_reader_error(e)),
                Err(RecvTimeoutError::Disconnected) => return Ok(all),
                Err(RecvTimeoutError::Timeout) => return Err(Error::Timeout("the server stopped answering")),
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

fn map_reader_error(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::Other {
        Error::Protocol(e.to_string())
    } else {
        Error::Io(e)
    }
}

/// Reader thread: HTTP head, then chunked 16-bit PCM decoded to f32 and forwarded. An HTTP error becomes an
/// `ErrorKind::Other` item carrying the status and body.
fn read_output(stream: TcpStream, tx: mpsc::Sender<io::Result<Vec<f32>>>) {
    let mut reader = BufReader::new(stream);
    let run = |reader: &mut BufReader<TcpStream>| -> io::Result<()> {
        let head = http::read_head(reader)?;
        if head.status != 200 {
            let mut body = String::new();
            let mut raw = Vec::new();
            if head.chunked {
                while let Some(c) = http::read_chunk(reader)? {
                    raw.extend_from_slice(&c);
                }
            } else if let Some(n) = head.content_length {
                raw.resize(n, 0);
                io::Read::read_exact(reader, &mut raw)?;
            }
            body.push_str(&String::from_utf8_lossy(&raw));
            return Err(io::Error::other(format!("http {}: {body}", head.status)));
        }
        let mut odd: Option<u8> = None;
        while let Some(chunk) = http::read_chunk(reader)? {
            let mut bytes = chunk;
            if let Some(b) = odd.take() {
                bytes.insert(0, b);
            }
            if bytes.len() % 2 == 1 {
                odd = bytes.pop();
            }
            let samples: Vec<f32> =
                bytes.as_chunks::<2>().0.iter().map(|p| i16::from_le_bytes(*p) as f32 / 32768.0).collect();
            if !samples.is_empty() && tx.send(Ok(samples)).is_err() {
                return Ok(());
            }
        }
        Ok(())
    };
    if let Err(e) = run(&mut reader) {
        let _ = tx.send(Err(e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_json_escapes_and_sets_options() {
        let mut c = Config::new("/bin/x".into(), "/m/a \"b\".gguf".into(), "/tmp/w".into());
        c.lookahead_frames = Some(4);
        c.backend = Backend::Cpu;
        let j = c.server_json(1234);
        assert!(j.contains(r#""port":1234"#) && j.contains(r#""backend":"cpu""#));
        assert!(j.contains(r#""path":"/m/a \"b\".gguf""#));
        assert!(j.contains(r#""voice_isolator.lookahead_frames":4"#));
    }

    #[test]
    fn relative_binary_is_refused() {
        let c = Config::new("audiocpp_server".into(), "/m".into(), std::env::temp_dir().join("kalango-audiocpp-rel"));
        assert!(matches!(Server::spawn(&c), Err(Error::Io(e)) if e.kind() == io::ErrorKind::InvalidInput));
    }

    #[test]
    fn a_missing_binary_fails_cleanly() {
        let c = Config::new(
            "/nonexistent/audiocpp_server".into(),
            "/m".into(),
            std::env::temp_dir().join("kalango-audiocpp-miss"),
        );
        assert!(matches!(Server::spawn(&c), Err(Error::Io(_))));
    }
}
