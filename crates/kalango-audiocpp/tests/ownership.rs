//! The wrapper must only ever talk to the socket of the child it spawned.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use kalango_audiocpp::{Config, Error, Server};

fn fake_config(tag: &str) -> Config {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_fake_audiocpp_server"));
    let dir = std::env::temp_dir().join(format!("kalango-audiocpp-own-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = Config::new(bin, "/unused".into(), dir);
    cfg.startup_timeout = Duration::from_secs(10);
    cfg
}

#[test]
fn talks_to_its_own_child_on_a_kernel_chosen_port() {
    let cfg = fake_config("own");
    let server = Server::spawn(&cfg).expect("spawn");
    assert_ne!(server.addr().port(), 0);
    server.health().expect("health");
    server.stop(Duration::from_secs(2)).expect("stop");
    let _ = std::fs::remove_dir_all(&cfg.work_dir);
}

#[test]
fn a_rogue_listener_is_never_trusted_or_contacted() {
    // a rogue "audio service" that answers 200 to anything and counts connections
    let rogue = TcpListener::bind("127.0.0.1:0").unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in rogue.incoming() {
            let Ok(mut stream) = stream else { continue };
            counter.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    // the child never listens, so nothing on this machine can be "its" port: spawn must time out, not succeed
    let mut cfg = fake_config("rogue");
    cfg.env.push(("FAKE_NO_LISTEN".into(), "1".into()));
    cfg.startup_timeout = Duration::from_millis(1500);
    let started = std::time::Instant::now();
    match Server::spawn(&cfg) {
        Err(Error::Timeout(_)) => {}
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("spawn trusted a socket that is not the child's"),
    }
    assert!(started.elapsed() < Duration::from_secs(10), "the child must be killed on the error path");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no request may reach the rogue listener");
    let _ = std::fs::remove_dir_all(&cfg.work_dir);
}

#[test]
fn an_open_work_directory_is_refused() {
    let cfg = fake_config("open");
    std::fs::create_dir_all(&cfg.work_dir).unwrap();
    std::fs::set_permissions(&cfg.work_dir, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();
    assert!(matches!(Server::spawn(&cfg), Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::PermissionDenied));
    let _ = std::fs::remove_dir_all(&cfg.work_dir);
}
