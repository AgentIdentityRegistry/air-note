//! M5b sandbox: the jailed subprocess I/O pump + process-group kill. The
//! authoritative resource guarantees (no hang, bounded output) live here on the
//! Rust side; the OS jail (T6) and egress probe (T7) build on `run_pump`.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt; // process_group — safe, stable
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ingest::IngestError;

/// Run `cmd` in its own process group, streaming `input` to stdin on a writer
/// thread (no deadlock against a full stdout pipe), reading stdout incrementally
/// under `out_cap` (killing the group the instant the cap is exceeded), reading
/// stderr into a bounded buffer, and enforcing `timeout` with a group-kill.
/// EVERY return path reaps the child. Returns stdout as UTF-8.
#[allow(dead_code)] // used by pump_tests and by future T8 parser
pub(crate) fn run_pump(
    mut cmd: Command,
    input: &[u8],
    out_cap: usize,
    stderr_cap: usize,
    timeout: Duration,
) -> Result<String, IngestError> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.process_group(0); // child leads its own group → group-kill reaps helpers
    let mut child = cmd
        .spawn()
        .map_err(|e| IngestError::SandboxUnavailable(format!("spawn: {e}")))?;
    let pid = child.id() as i32;

    let mut stdin = child.stdin.take().expect("piped");
    let input_owned = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input_owned); // EPIPE if the child died: ignore
    });

    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out_reader = std::thread::spawn(move || read_capped(&mut stdout, out_cap, pid));
    let err_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = (&mut stderr).take(stderr_cap as u64).read_to_end(&mut b);
        b
    });

    let timed_out = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    #[allow(unused_assignments)] // initial None is overwritten in loop before use
    let mut status = None;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => { status = Some(s); break; }
            Ok(None) if started.elapsed() >= timeout => {
                timed_out.store(true, Ordering::SeqCst);
                kill_group(pid);
                status = child.wait().ok();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => { kill_group(pid); status = child.wait().ok(); break; }
        }
    }

    let _ = writer.join();
    let out = out_reader.join().unwrap_or(Err(()));
    let err_bytes = err_reader.join().unwrap_or_default();

    if timed_out.load(Ordering::SeqCst) {
        return Err(IngestError::Timeout);
    }
    match out {
        Err(()) => Err(IngestError::Parse("output cap exceeded".into())),
        Ok(bytes) => match status {
            Some(s) if s.success() => String::from_utf8(bytes).map_err(|_| IngestError::NonUtf8),
            _ => {
                let tail = String::from_utf8_lossy(&err_bytes);
                let tail = tail.trim();
                Err(IngestError::Parse(format!("markitdown failed: {}", &tail[..tail.len().min(200)])))
            }
        },
    }
}

/// Read from `r` into a Vec, killing the process group `pid` and returning
/// `Err(())` the moment the read would exceed `cap` (so a flooding child is
/// stopped immediately, not at the wall-clock timeout).
#[allow(dead_code)] // called from thread closures inside run_pump
fn read_capped(r: &mut impl Read, cap: usize, pid: i32) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => return Ok(buf),
            Ok(n) => {
                if buf.len() + n > cap {
                    kill_group(pid);
                    return Err(());
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(_) => return Ok(buf), // pipe closed (e.g. after a kill)
        }
    }
}

/// Kill the whole process group led by `pid` (safe rustix wrapper; no `unsafe`).
/// `Pid::from_raw` returns `None` for 0, so this can never target our own group.
#[allow(dead_code)] // called from run_pump loop arms; future T6/T7 will also call it
fn kill_group(pid: i32) {
    if let Some(p) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process_group(p, rustix::process::Signal::Kill);
    }
}

#[cfg(test)]
mod pump_tests {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    fn sh(script: &str) -> Command { let mut c = Command::new("/bin/sh"); c.arg("-c").arg(script); c }

    #[test]
    fn pump_streams_stdin_to_stdout() {
        let out = run_pump(sh("cat"), b"hello", 1 << 20, 64 << 10, Duration::from_secs(5)).unwrap();
        assert_eq!(out, "hello");
    }
    #[test]
    fn pump_kills_on_timeout() {
        let err = run_pump(sh("sleep 30"), b"", 1 << 20, 64 << 10, Duration::from_millis(300)).unwrap_err();
        assert!(matches!(err, crate::ingest::IngestError::Timeout), "got {err:?}");
    }
    #[test]
    fn pump_enforces_output_cap() {
        // `yes` floods forever; the reader must kill at the cap, NOT wait for the 5s timeout.
        let err = run_pump(sh("yes aaaaaaaa"), b"", 4096, 64 << 10, Duration::from_secs(5)).unwrap_err();
        assert!(matches!(err, crate::ingest::IngestError::Parse(_)), "got {err:?}");
    }
    #[test]
    fn pump_does_not_deadlock_on_large_interleaved_io() {
        let out = run_pump(sh("cat"), &vec![b'x'; 1 << 20], 4 << 20, 64 << 10, Duration::from_secs(10)).unwrap();
        assert_eq!(out.len(), 1 << 20);
    }
}
