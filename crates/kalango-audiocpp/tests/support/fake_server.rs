//! Stand-in for `audiocpp_server` in tests: binds 127.0.0.1 on the port of the generated `--config` JSON.
//!
//! - `GET /health` answers `{"status":"ok","backend":"<the configured backend>"}`; any other GET answers 200 "ok".
//! - `POST /v1/audio/speech/live` is a fake "isolator": chunked f32le in, chunked 16-bit PCM out, every sample
//!   multiplied by a gain, so tests can tell the output from the input and check the alignment.
//!
//! Behaviour switches, all through the environment the test gives the child:
//! - `FAKE_NO_LISTEN=1`: never listens (so any answer on any port cannot be its own).
//! - `FAKE_GAIN=0.5`: the gain of the fake isolator.
//! - `FAKE_LAG=2048`: samples held back, like the real model's algorithmic delay (flushed when the input ends).
//! - `FAKE_MS_PER_CHUNK=30`: milliseconds spent on every input chunk (a server slower than real time).
//! - `FAKE_BACKEND=cpu`: the backend reported by `/health`, whatever the configuration says.
//! - `FAKE_DIE_AFTER_BYTES=N`: exits abruptly once N bytes of audio arrived (a crash while streaming).
//! - `FAKE_DIE_ONCE_FILE=path`: with `FAKE_DIE_AFTER_BYTES`, dies only if the file does not exist yet and creates it
//!   (so the restarted server behaves).
//! - `FAKE_START_DELAY_MS=300`: waits before listening.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn main() {
    if std::env::var_os("FAKE_NO_LISTEN").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    // `--config <path>`: take the port from the generated JSON like the real server does
    let path = std::env::args().nth(2).expect("config path");
    let json = std::fs::read_to_string(path).expect("config");
    let port: u16 =
        json.split("\"port\":").nth(1).and_then(|r| r.split(',').next()).and_then(|v| v.parse().ok()).expect("port");
    let backend = std::env::var("FAKE_BACKEND").unwrap_or_else(|_| {
        json.split("\"backend\":\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("cpu").to_string()
    });
    if let Some(ms) = env_usize("FAKE_START_DELAY_MS") {
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let backend = backend.clone();
        std::thread::spawn(move || handle(stream, &backend));
    }
}

fn handle(stream: TcpStream, backend: &str) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    // the rest of the head
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim_end().is_empty() {
            break;
        }
    }
    if request_line.starts_with("POST /v1/audio/speech/live") {
        live(&mut reader, &mut writer);
        return;
    }
    let body = if request_line.starts_with("GET /health") {
        format!("{{\"status\":\"ok\",\"backend\":\"{backend}\",\"models\":1}}")
    } else {
        "ok".to_string()
    };
    let _ = write!(writer, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

fn read_chunk(reader: &mut BufReader<TcpStream>) -> Option<Vec<u8>> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let size = usize::from_str_radix(line.trim().split(';').next()?, 16).ok()?;
    if size == 0 {
        let _ = reader.read_line(&mut line);
        return None;
    }
    let mut data = vec![0u8; size];
    reader.read_exact(&mut data).ok()?;
    let mut crlf = [0u8; 2];
    reader.read_exact(&mut crlf).ok()?;
    Some(data)
}

fn send_chunk(writer: &mut TcpStream, samples: &[f32], gain: f32) -> bool {
    if samples.is_empty() {
        return true;
    }
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&(((s * gain).clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    write!(writer, "{:x}\r\n", bytes.len()).is_ok()
        && writer.write_all(&bytes).is_ok()
        && writer.write_all(b"\r\n").is_ok()
}

fn live(reader: &mut BufReader<TcpStream>, writer: &mut TcpStream) {
    let gain = env_f32("FAKE_GAIN", 0.5);
    let lag = env_usize("FAKE_LAG").unwrap_or(0);
    let per_chunk = env_usize("FAKE_MS_PER_CHUNK").unwrap_or(0);
    let die_after = env_usize("FAKE_DIE_AFTER_BYTES");
    let die_once = std::env::var("FAKE_DIE_ONCE_FILE").ok();
    if write!(writer, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").is_err() {
        return;
    }
    let mut held: Vec<f32> = Vec::new();
    let mut received = 0usize;
    while let Some(chunk) = read_chunk(reader) {
        received += chunk.len();
        if let Some(limit) = die_after {
            let first_time = die_once.as_ref().map(|f| !std::path::Path::new(f).exists()).unwrap_or(true);
            if received >= limit && first_time {
                if let Some(f) = &die_once {
                    let _ = std::fs::write(f, b"died");
                }
                std::process::exit(9);
            }
        }
        if per_chunk > 0 {
            std::thread::sleep(std::time::Duration::from_millis(per_chunk as u64));
        }
        held.extend(chunk.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
        if held.len() > lag {
            let ready: Vec<f32> = held.drain(..held.len() - lag).collect();
            if !send_chunk(writer, &ready, gain) {
                return;
            }
        }
    }
    // the input ended: flush what the model still held, then end the body
    if send_chunk(writer, &held, gain) {
        let _ = writer.write_all(b"0\r\n\r\n");
    }
}
