//! A child process owned through a pidfd: signals cannot reach a recycled pid, and Drop always reaps.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub struct Process {
    child: Child,
    pidfd: OwnedFd,
}

fn code_of(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

impl Process {
    /// Spawns `program` (absolute path, no shell) with a cleared environment plus `env`, stdout and stderr appended to
    /// `log`. The child gets SIGKILL if this process dies (PR_SET_PDEATHSIG).
    pub fn spawn(program: &Path, args: &[String], env: &[(String, String)], log: &Path) -> io::Result<Self> {
        if !program.is_absolute() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the program path must be absolute"));
        }
        let log_out = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
        let log_err = log_out.try_clone()?;
        let mut cmd = Command::new(program);
        cmd.args(args).env_clear().envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null());
        cmd.stdout(Stdio::from(log_out)).stderr(Stdio::from(log_err));
        // SAFETY: only async-signal-safe libc calls (prctl) run between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        // SAFETY: plain syscall with a pid we just created.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id() as libc::pid_t, 0) };
        if fd < 0 {
            let err = io::Error::last_os_error();
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        // SAFETY: pidfd_open returned a fresh descriptor that nothing else owns.
        let pidfd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        Ok(Self { child, pidfd })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    fn signal(&self, sig: libc::c_int) -> io::Result<()> {
        // SAFETY: the pidfd is valid for the life of self; null siginfo and zero flags are allowed.
        let r = unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, self.pidfd.as_raw_fd(), sig, std::ptr::null::<()>(), 0)
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// `Some(code)` once the process exited (a signal death reports 128+signal), `None` while it runs.
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        Ok(self.child.try_wait()?.map(code_of))
    }

    /// SIGTERM, wait up to `grace`, then SIGKILL; always reaps. Returns the exit code.
    pub fn stop(&mut self, grace: Duration) -> io::Result<i32> {
        if let Some(code) = self.try_wait()? {
            return Ok(code);
        }
        let _ = self.signal(libc::SIGTERM);
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if let Some(code) = self.try_wait()? {
                return Ok(code);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.signal(libc::SIGKILL);
        Ok(code_of(self.child.wait()?))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.signal(libc::SIGKILL);
        let _ = self.child.wait();
    }
}
