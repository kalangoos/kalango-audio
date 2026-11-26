//! Stand-in for `audiocpp_server` in tests: binds 127.0.0.1:0 and answers every request 200 "ok".
//! With `FAKE_NO_LISTEN=1` it never listens (so any answer on any port cannot be its own).

use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    if std::env::var_os("FAKE_NO_LISTEN").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        // read the whole request head, or closing early would reset the connection
        let (mut seen, mut buf) = (Vec::new(), [0u8; 256]);
        while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        }
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    }
}
