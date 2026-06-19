//! M5b sandbox: the jailed subprocess I/O pump + process-group kill. The
//! authoritative resource guarantees (no hang, bounded output) live here on the
//! Rust side; the OS jail (T6) and egress probe (T7) build on `run_pump`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::process::CommandExt; // process_group — safe, stable
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ingest::{IngestError, Parser, PathHint};

/// Run `cmd` in its own process group, streaming `input` to stdin on a writer
/// thread (no deadlock against a full stdout pipe), reading stdout incrementally
/// under `out_cap` (killing the group the instant the cap is exceeded), reading
/// stderr into a bounded buffer, and enforcing `timeout` with a group-kill.
/// EVERY return path reaps the child. Returns stdout as UTF-8.
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

use std::path::{Path, PathBuf};

/// A located, validated venv: the interpreter + the first-party wrapper.
#[derive(Debug)]
pub(crate) struct Venv {
    pub(crate) python: PathBuf,
    pub(crate) wrapper: PathBuf,
}

/// Locate the bundled markitdown venv via an explicit path. Env override for
/// tests/headless (`BOSSCLAW_MARKITDOWN_VENV`); the desktop wires the
/// app-resources path in M7. Missing/invalid → `SandboxUnavailable` (→ skip).
pub(crate) fn discover_venv() -> Result<Venv, IngestError> {
    let root = std::env::var_os("BOSSCLAW_MARKITDOWN_VENV")
        .map(PathBuf::from)
        .ok_or_else(|| IngestError::SandboxUnavailable("no venv path configured".into()))?;
    let python = root.join("bin").join("python");
    let wrapper = root.join("convert_stdin.py");
    if !python.exists() || !wrapper.exists() {
        return Err(IngestError::SandboxUnavailable(format!("venv incomplete at {}", root.display())));
    }
    Ok(Venv { python, wrapper })
}

/// Scrub the child's environment + pin its cwd to the scratch dir. SHARED by the
/// jailed builder AND the test builder so there is exactly ONE scrub path (no
/// drift between what tests check and what production runs). No secret (DEK,
/// signing key, API keys) can reach the child via env — `env_clear` then a
/// minimal allowlist.
fn apply_scrub(c: &mut Command, scratch: &Path) {
    c.env_clear();
    c.env("PATH", "/usr/bin:/bin");
    c.env("LC_ALL", "C.UTF-8");
    c.env("HOME", scratch);
    c.env("PYTHONNOUSERSITE", "1");
    c.env("PYTHONDONTWRITEBYTECODE", "1");
    c.env("PYTHONHASHSEED", "0");
    c.current_dir(scratch);
}

/// Test-only builder: env-scrubbed + scratch cwd, NO OS jail. Exercises the same
/// `apply_scrub` production uses, so the scrub test covers the real scrub path.
#[cfg(test)]
fn build_jailed_command_for_test(scratch: &Path, program: &str, args: &[String]) -> Command {
    let mut c = Command::new(program);
    c.args(args);
    apply_scrub(&mut c, scratch);
    c
}

/// Wrap `program`+`args` with the per-OS network+fs jail.
///
/// macOS: `sandbox-exec` + a Seatbelt profile that denies network + denies writes outside
/// the scratch (file-read is broad — spec posture is network-hard, fs-read best-effort).
/// Linux: `bwrap` (unshare net+pid+ipc, ro-bind system + the venv, tmpfs/bind scratch).
/// EFFICACY is proven by the T7 egress probe. `program` is the venv python (so its
/// parent's parent is the venv root, which Linux must bind).
#[cfg(target_os = "macos")]
fn wrap_jail(scratch: &Path, program: &str, args: &[String]) -> Command {
    let mut c = Command::new("/usr/bin/sandbox-exec");
    c.arg("-D").arg(format!("SCRATCH={}", scratch.display()));
    c.arg("-p").arg(include_str!("sandbox_profiles/seatbelt.sb"));
    c.arg(program).args(args);
    apply_scrub(&mut c, scratch);
    c
}
#[cfg(target_os = "linux")]
fn wrap_jail(scratch: &Path, program: &str, args: &[String]) -> Command {
    let venv_root = Path::new(program).parent().and_then(|p| p.parent());
    let mut c = Command::new("bwrap");
    c.args([
        "--unshare-net", "--unshare-pid", "--unshare-ipc", "--die-with-parent",
        "--ro-bind-try", "/usr", "/usr", "--ro-bind-try", "/bin", "/bin",
        "--ro-bind-try", "/lib", "/lib", "--ro-bind-try", "/lib64", "/lib64",
        "--proc", "/proc", "--dev", "/dev",
    ]);
    if let Some(root) = venv_root {
        let r = root.to_string_lossy().into_owned();
        c.arg("--ro-bind-try").arg(&r).arg(&r);
    }
    c.arg("--bind").arg(scratch).arg(scratch).arg("--chdir").arg(scratch);
    c.arg(program).args(args);
    apply_scrub(&mut c, scratch);
    c
}

// ── Egress probe ────────────────────────────────────────────────────────────

#[derive(Debug)]
#[allow(dead_code)] // diagnostic payload, read via Debug
enum ProbeOutcome {
    Connected,
    RefusedPerm,
    RefusedConn,
    Other(String),
    ChildError(String),
}

/// True iff the jail genuinely denies network: the jailed child's connect to our
/// loopback listener is refused at the jail layer AND nothing is accepted.
/// Fail-closed: any other outcome → false (not proven → caller skips).
pub(crate) fn probe_egress(venv: &Venv) -> bool {
    run_probe(venv, true)
}

fn run_probe(venv: &Venv, jailed: bool) -> bool {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(_) => return false,
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(_) => return false,
    };
    // Dedicated accept thread, bounded — ANY accepted socket means the jail FAILED.
    let acc = std::thread::spawn(move || accept_within(&listener, Duration::from_secs(4)));

    let scratch = match tempfile::tempdir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    let py = match venv.python.to_str() {
        Some(s) => s,
        None => return false,
    };
    // The child tries to connect and prints exactly one sentinel.
    let script = format!(
        "import socket,sys,errno\n\
         try:\n\
        \x20   socket.create_connection(('127.0.0.1',{port}),timeout=2); print('CONNECTED')\n\
         except OSError as e:\n\
        \x20   pm=(errno.EPERM,errno.EACCES,errno.ENETUNREACH,errno.EAFNOSUPPORT)\n\
        \x20   print('REFUSED_PERM' if e.errno in pm else ('REFUSED_CONN' if e.errno==errno.ECONNREFUSED else 'OTHER:%d'%(e.errno or -1)))"
    );
    let cmd = if jailed {
        wrap_jail(scratch.path(), py, &["-c".into(), script])
    } else {
        let mut c = Command::new(py);
        c.args(["-c", &script]);
        apply_scrub(&mut c, scratch.path());
        c
    };
    let outcome = match run_pump(cmd, b"", 1 << 16, 64 << 10, Duration::from_secs(8)) {
        Ok(s) => match s.trim() {
            "CONNECTED" => ProbeOutcome::Connected,
            "REFUSED_PERM" => ProbeOutcome::RefusedPerm,
            "REFUSED_CONN" => ProbeOutcome::RefusedConn,
            o => ProbeOutcome::Other(o.to_string()),
        },
        Err(e) => ProbeOutcome::ChildError(format!("{e:?}")),
    };
    let accepted = acc.join().unwrap_or(false);
    // PROVEN only on a jail-layer refusal with nothing accepted. Everything else
    // is fail-closed (not proven). Logged so the controller can audit honesty.
    eprintln!("[egress-probe] jailed={jailed} outcome={outcome:?} accepted={accepted}");
    matches!(outcome, ProbeOutcome::RefusedPerm) && !accepted
}

/// Block up to `timeout` for a single inbound connection; true iff one arrives.
fn accept_within(listener: &TcpListener, timeout: Duration) -> bool {
    listener.set_nonblocking(true).ok();
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok(_) => return true,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= timeout {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return false,
        }
    }
}

// ── SandboxedMarkitdownParser ────────────────────────────────────────────────

/// Pinned markitdown version — MUST equal the venv lockfile pin (drift-guarded in CI).
const MARKITDOWN_VERSION: &str = "0.1.6";
const RICH_OUTPUT_CAP: usize = 32 * 1024 * 1024;
const RICH_STDERR_CAP: usize = 64 * 1024;
const RICH_WALL_CLOCK: Duration = Duration::from_secs(30);

/// The sandboxed rich-document parser (M5b brick 1). Proves the network jail
/// once at construction (fail-closed — `discover` returns `SandboxUnavailable` if
/// the jail can't be proven, so a constructed parser is always jailed). Rebuilds
/// a fresh jailed child per `convert`; `run_pump`'s spawn-error path fails closed
/// if the jail tool is missing at spawn time.
pub struct SandboxedMarkitdownParser {
    venv: Venv,
    id: String,
}

impl SandboxedMarkitdownParser {
    /// Locate the venv and prove the jail. Returns `SandboxUnavailable` (→ skip)
    /// if either fails — never constructs an un-jailed parser.
    pub fn discover() -> Result<Self, IngestError> {
        let venv = discover_venv()?;
        if !probe_egress(&venv) {
            return Err(IngestError::SandboxUnavailable(
                "network jail could not be proven".into(),
            ));
        }
        Ok(Self {
            venv,
            id: format!("markitdown-sandboxed-v{MARKITDOWN_VERSION}"),
        })
    }
}

impl Parser for SandboxedMarkitdownParser {
    fn convert(&self, raw: &[u8], hint: &PathHint) -> Result<String, IngestError> {
        let scratch =
            tempfile::tempdir().map_err(|e| IngestError::Io(e.to_string()))?;
        let ext = hint.ext.clone().unwrap_or_default();
        let py = self
            .venv
            .python
            .to_str()
            .ok_or_else(|| IngestError::SandboxUnavailable("non-utf8 venv python path".into()))?;
        let wrapper = self.venv.wrapper.to_string_lossy().into_owned();
        let cmd = wrap_jail(scratch.path(), py, &[wrapper, ext]);
        run_pump(cmd, raw, RICH_OUTPUT_CAP, RICH_STDERR_CAP, RICH_WALL_CLOCK)
        // `scratch` drops here on every path (success/timeout/cap) → removed.
    }

    fn parser_id(&self) -> &str {
        &self.id
    }
}

/// TEST ONLY. Spawn a jailed child that fstat-scans fds 3..64 and reports any
/// that are open regular files — the parent's SQLCipher DB handle would appear
/// here if it were NOT O_CLOEXEC. Returns the leaked fd numbers (MUST be empty).
#[cfg(test)]
pub(crate) fn spawn_jailed_fd_scan() -> Vec<i32> {
    let venv = match discover_venv() { Ok(v) => v, Err(_) => return vec![-1] };
    let scratch = match tempfile::tempdir() { Ok(d) => d, Err(_) => return vec![-2] };
    let py = match venv.python.to_str() { Some(s) => s, None => return vec![-3] };
    let script = "import os,stat\nleaked=[]\nfor fd in range(3,64):\n    try:\n        st=os.fstat(fd)\n        if stat.S_ISREG(st.st_mode): leaked.append(fd)\n    except OSError:\n        pass\nprint('LEAKED:'+','.join(map(str,leaked)))";
    let cmd = wrap_jail(scratch.path(), py, &["-c".into(), script.into()]);
    let out = run_pump(cmd, b"", 1 << 16, 64 << 10, std::time::Duration::from_secs(8)).unwrap_or_default();
    out.lines()
        .find_map(|l| l.strip_prefix("LEAKED:"))
        .map(|s| s.split(',').filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_default()
}

/// TEST ONLY. Build the parser, convert `bytes` (with `{PORT}` replaced by a live
/// loopback listener's port), and report whether ANY outbound connection was made.
/// MUST be false — the stripped registry has no URL-fetching converter and the jail
/// denies network. Returns true (= FAIL) if anything connected.
#[cfg(feature = "sandbox-test-hooks")]
pub(crate) fn convert_makes_outbound_connection(bytes_template: &str, ext: &str) -> bool {
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") { Ok(l) => l, Err(_) => return false };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let acc = std::thread::spawn(move || accept_within(&listener, std::time::Duration::from_secs(4)));
    let connected = (|| {
        let p = SandboxedMarkitdownParser::discover().ok()?;
        let bytes = bytes_template.replace("{PORT}", &port.to_string());
        let _ = p.convert(bytes.as_bytes(), &crate::ingest::PathHint { ext: Some(ext.to_string()) });
        Some(())
    })().is_some();
    let accepted = acc.join().unwrap_or(false);
    let _ = connected;
    accepted
}

/// Public test hooks — called from `tests/sandbox.rs` integration tests.
#[cfg(feature = "sandbox-test-hooks")]
pub mod sandbox_test_hooks {
    /// True iff the jail proves network denial (jailed connect refused at jail layer).
    pub fn probe_egress_blocks() -> bool {
        super::discover_venv()
            .map(|v| super::probe_egress(&v))
            .unwrap_or(false)
    }
    /// True iff even the UN-jailed child fails to connect (should be FALSE — it
    /// proves the probe has teeth: without the jail, the connect succeeds).
    pub fn unjailed_probe_blocks() -> bool {
        super::discover_venv()
            .map(|v| super::run_probe(&v, false))
            .unwrap_or(false)
    }
    /// True iff converting `bytes_template` (a hostile document) caused an outbound
    /// TCP connection to a loopback listener. MUST be false — the jail must block it.
    pub fn hostile_doc_connects(bytes_template: &str, ext: &str) -> bool {
        super::convert_makes_outbound_connection(bytes_template, ext)
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

    #[test]
    fn run_pump_fails_closed_when_program_missing() {
        let err = run_pump(Command::new("/nonexistent/jail-tool-xyz"), b"", 1 << 16, 64 << 10, Duration::from_secs(2)).unwrap_err();
        assert!(matches!(err, crate::ingest::IngestError::SandboxUnavailable(_)), "missing tool must fail closed, got {err:?}");
    }
}

#[cfg(test)]
mod jail_tests {
    use super::*;

    #[test]
    fn discover_venv_missing_env_is_sandbox_unavailable() {
        // Ensure the var is unset for this test (it may be set in the env).
        std::env::remove_var("BOSSCLAW_MARKITDOWN_VENV");
        assert!(matches!(discover_venv().unwrap_err(), crate::ingest::IngestError::SandboxUnavailable(_)));
    }

    #[test]
    fn discover_venv_incomplete_dir_is_sandbox_unavailable() {
        let d = tempfile::tempdir().unwrap();
        std::env::set_var("BOSSCLAW_MARKITDOWN_VENV", d.path()); // empty dir → no python/wrapper
        let r = discover_venv();
        std::env::remove_var("BOSSCLAW_MARKITDOWN_VENV");
        assert!(matches!(r.unwrap_err(), crate::ingest::IngestError::SandboxUnavailable(_)));
    }

    #[test]
    fn apply_scrub_clears_env_and_sets_scratch_cwd() {
        std::env::set_var("FAKE_SECRET", "leak-me");
        let scratch = tempfile::tempdir().unwrap();
        let cmd = build_jailed_command_for_test(
            scratch.path(),
            "/bin/sh",
            &["-c".into(), "echo \"${FAKE_SECRET:-CLEAN}\"; pwd".into()],
        );
        let out = run_pump(cmd, b"", 1 << 20, 64 << 10, std::time::Duration::from_secs(5)).unwrap();
        std::env::remove_var("FAKE_SECRET");
        assert!(out.contains("CLEAN"), "env must be scrubbed; got: {out}");
        assert!(
            out.contains(&*scratch.path().to_string_lossy()),
            "cwd must be the scratch dir; got: {out}"
        );
    }
}
