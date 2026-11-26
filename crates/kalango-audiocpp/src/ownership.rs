//! Proof that the socket we talk to belongs to the child we spawned, and a private directory for its files.
//!
//! The server is started on port 0 (the kernel picks a free port, there is no release-and-reuse window) and the port it
//! got is read back from the kernel: the child's own listening socket inodes (`/proc/<pid>/fd`) matched against the
//! LISTEN rows of `/proc/<pid>/net/tcp`. A rogue process on another port, or one that races for ours, is never trusted.

use std::collections::HashSet;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The loopback TCP port that process `pid` is listening on, if any.
pub fn listening_port_of(pid: u32) -> io::Result<Option<u16>> {
    let mut inodes = HashSet::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
        let Ok(target) = std::fs::read_link(entry?.path()) else { continue };
        if let Some(inode) = target.to_string_lossy().strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')) {
            if let Ok(inode) = inode.parse::<u64>() {
                inodes.insert(inode);
            }
        }
    }
    let table = std::fs::read_to_string(format!("/proc/{pid}/net/tcp"))?;
    Ok(find_listener(&table, &inodes))
}

/// Parses `/proc/net/tcp` text: a LISTEN row (state 0A) on 127.0.0.1 whose inode is in `inodes`.
fn find_listener(table: &str, inodes: &HashSet<u64>) -> Option<u16> {
    for row in table.lines().skip(1) {
        let cols: Vec<&str> = row.split_whitespace().collect();
        if cols.len() < 10 || cols[3] != "0A" {
            continue;
        }
        let Some((addr, port)) = cols[1].split_once(':') else { continue };
        // 127.0.0.1 is stored little endian
        if addr != "0100007F" || !cols[9].parse::<u64>().is_ok_and(|i| inodes.contains(&i)) {
            continue;
        }
        if let Ok(port) = u16::from_str_radix(port, 16) {
            return Some(port);
        }
    }
    None
}

/// Creates `dir` (and parents) as a private 0700 directory, or accepts an existing one only if it is owned by us and
/// closed to group and others. Prefer a directory under `XDG_RUNTIME_DIR`.
pub fn private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != me || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} must be a directory owned by this user with mode 0700", dir.display()),
        ));
    }
    Ok(())
}

/// Writes `contents` to `dir/name` as a fresh 0600 file (replacing a previous one), never through a symlink.
pub fn write_private(dir: &Path, name: &str, contents: &str) -> io::Result<PathBuf> {
    let path = dir.join(name);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    io::Write::write_all(&mut file, contents.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parses_only_matching_loopback_listeners() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
            0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0 100 0 0 10 0\n\
            1: 0100007F:2328 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222 1 0 100 0 0 10 0\n\
            2: 0100007F:2329 0100007F:9999 01 00000000:00000000 00:00000000 00000000  1000        0 333 1 0 100 0 0 10 0\n\
            3: 00000000:2710 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 444 1 0 100 0 0 10 0\n";
        let mine: HashSet<u64> = [222u64, 333, 444].into();
        assert_eq!(find_listener(table, &mine), Some(0x2328)); // not 111 (rogue), not 333 (ESTABLISHED), not 444 (0.0.0.0)
        assert_eq!(find_listener(table, &HashSet::from([999u64])), None);
    }

    #[test]
    fn finds_this_process_listener_but_not_another_process() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(listening_port_of(std::process::id()).unwrap(), Some(port));
        // a child that listens on nothing must not inherit this process's listener
        let mut other = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let found = listening_port_of(other.id()).unwrap();
        let _ = other.kill();
        let _ = other.wait();
        assert_eq!(found, None);
    }

    #[test]
    fn private_dir_rejects_open_permissions() {
        let base = std::env::temp_dir().join(format!("kalango-audiocpp-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("ok");
        private_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        let p = write_private(&dir, "c.json", "{}").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().mode() & 0o777, 0o600);
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert!(private_dir(&dir).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
