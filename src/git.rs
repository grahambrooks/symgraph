//! Running `git` as a subprocess, with a bound on how long it may take.
//!
//! Every symgraph tool that shells out to git — blame, churn, diff-impact,
//! index-location discovery — does so while the caller waits, and several do
//! it while holding the database lock. A git that never returns (a credential
//! prompt on a private remote, a stalled network filesystem, an enormous
//! history window) would otherwise hang that tool call forever and, over HTTP,
//! every other session with it.
//!
//! [`run_git`] is the single entry point: it spawns git, drains both pipes on
//! their own threads so a full pipe buffer cannot deadlock the wait, and kills
//! the child once the deadline passes.

use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long any single git invocation may run before it is killed.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the wait loop checks whether git has exited.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A finished git invocation. Mirrors the parts of `std::process::Output` the
/// callers actually use.
pub struct GitOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GitOutput {
    /// `stderr`, lossily decoded and trimmed — the shape every caller wants
    /// when reporting a failed invocation.
    pub fn stderr_message(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Run `git <args>` in `cwd`, failing if it has not exited within [`GIT_TIMEOUT`].
///
/// `-c core.quotePath=false` is applied to every invocation so paths git prints
/// stay byte-for-byte comparable with the paths symgraph stores; git would
/// otherwise C-quote any path containing a non-ASCII byte.
pub fn run_git<I, S>(cwd: &Path, args: I) -> Result<GitOutput, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.quotePath=false")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| format!("running git: {}", e))?;

    // Drain both pipes on their own threads. Polling `try_wait` while git
    // fills a pipe buffer we never read would deadlock: git blocks on the
    // write, we block waiting for an exit that cannot come.
    let stdout_rx = drain(child.stdout.take());
    let stderr_rx = drain(child.stderr.take());

    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("git timed out after {}s", GIT_TIMEOUT.as_secs()));
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(format!("waiting for git: {}", e)),
        }
    };

    Ok(GitOutput {
        status,
        stdout: stdout_rx.recv().unwrap_or_default(),
        stderr: stderr_rx.recv().unwrap_or_default(),
    })
}

/// Read `pipe` to end on a worker thread, handing the bytes back over a channel.
/// A missing pipe or a read error yields empty output rather than failing the
/// invocation — git's exit status is what decides success.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cwd() -> PathBuf {
        std::env::current_dir().unwrap()
    }

    #[test]
    fn captures_stdout_of_a_successful_invocation() {
        let out = run_git(&cwd(), ["--version"]).unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("git version"));
    }

    #[test]
    fn reports_failure_status_with_stderr() {
        let out = run_git(&cwd(), ["rev-parse", "--verify", "definitely-not-a-ref"]).unwrap();
        assert!(!out.status.success());
        assert!(!out.stderr_message().is_empty());
    }

    #[test]
    fn large_output_does_not_deadlock() {
        // Far more than a pipe buffer: proves the drain threads are doing
        // their job rather than the wait loop stalling on a full pipe.
        let out = run_git(&cwd(), ["log", "--pretty=format:%H %s"]).unwrap();
        assert!(out.status.success());
    }
}
