use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;
pub use std::process::ExitStatus;

use anyhow::{Result, bail};

/// Error returned when a command exceeds its `max_time` limit.
#[derive(Debug)]
pub struct TimeoutError;

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "timed out")
    }
}

impl std::error::Error for TimeoutError {}

/// Wait for `child` to exit, killing it if `timeout` elapses first.
/// Returns `Err(TimeoutError)` if the process was killed due to timeout.
fn wait_with_timeout(child: &mut Child, timeout: Option<Duration>) -> Result<std::process::ExitStatus, TimeoutError> {
    let Some(limit) = timeout else {
        return child.wait().map_err(|_| TimeoutError);
    };

    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(_) => return Err(TimeoutError),
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TimeoutError);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run a command and capture its stdout+stderr combined into a single buffer.
/// Returns the captured bytes and an error if the command exits non-zero.
/// `dir` sets the working directory; `None` inherits the current directory.
/// `env` sets extra environment variables for this invocation only.
/// `timeout` kills the process if it runs longer than the given duration.
pub fn run_command(argv: &[String], dir: Option<&str>, env: &HashMap<String, String>, timeout: Option<Duration>) -> Result<Vec<u8>> {
    let (program, rest) = match argv {
        [] => return Ok(Vec::new()),
        [program, rest @ ..] => (program, rest),
    };

    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in env {
        cmd.env(k, v);
    }

    if let Some(d) = dir {
        cmd.current_dir(d);
    }

    if timeout.is_none() {
        // Fast path: no timeout, use the simpler blocking `.output()`.
        let Output { status, stdout, stderr } = cmd.output()?;
        let mut combined = stdout;
        combined.extend_from_slice(&stderr);
        if !status.success() {
            let code = status.code().map_or("signal".to_string(), |c| c.to_string());
            bail!("exited {code}: {}\n{}", argv.join(" "), String::from_utf8_lossy(&combined));
        }
        return Ok(combined);
    }

    let mut child = cmd.spawn()?;

    // Drain pipes in threads before waiting, otherwise the process can block
    // on a full pipe buffer and the timeout poll loop would never observe it exiting.
    let child_stdout = child.stdout.take().expect("stdout piped");
    let child_stderr = child.stderr.take().expect("stderr piped");

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = child_stdout;
        let _ = std::io::Read::read_to_end(&mut reader, &mut buf);
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut reader = child_stderr;
        let _ = std::io::Read::read_to_end(&mut reader, &mut buf);
        buf
    });

    let status = wait_with_timeout(&mut child, timeout)
        .map_err(|_| anyhow::anyhow!(TimeoutError))?;

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();

    // Interleave isn't possible post-hoc, so emit stderr after stdout.
    let mut combined = stdout;
    combined.extend_from_slice(&stderr);

    if !status.success() {
        let code = status
            .code()
            .map_or("signal".to_string(), |c| c.to_string());
        bail!(
            "exited {code}: {}\n{}",
            argv.join(" "),
            String::from_utf8_lossy(&combined)
        );
    }

    Ok(combined)
}

/// Run a command, streaming stdout+stderr directly to the terminal in real time.
/// Returns an error if the command exits non-zero; the output has already been
/// printed so the caller does not need to display it again.
/// `env` sets extra environment variables for this invocation only.
/// `timeout` kills the process if it runs longer than the given duration.
pub fn run_command_streaming(argv: &[String], dir: Option<&str>, env: &HashMap<String, String>, timeout: Option<Duration>) -> Result<()> {
    let (program, rest) = match argv {
        [] => return Ok(()),
        [program, rest @ ..] => (program, rest),
    };

    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in env {
        cmd.env(k, v);
    }

    if let Some(d) = dir {
        cmd.current_dir(d);
    }

    let mut child = cmd.spawn()?;

    // Drain both pipes concurrently using two threads so neither blocks.
    // We write directly to the locked stdout/stderr handles to avoid
    // per-byte locking overhead.
    let child_stdout = child.stdout.take().expect("stdout piped");
    let child_stderr = child.stderr.take().expect("stderr piped");

    let stdout_thread = std::thread::spawn(move || {
        let mut reader = child_stdout;
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = out.write_all(&buf[..n]);
                    let _ = out.flush();
                }
                Err(_) => break,
            }
        }
    });

    let stderr_thread = std::thread::spawn(move || {
        let mut reader = child_stderr;
        let stderr = std::io::stderr();
        let mut err = stderr.lock();
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = err.write_all(&buf[..n]);
                    let _ = err.flush();
                }
                Err(_) => break,
            }
        }
    });

    let status = wait_with_timeout(&mut child, timeout)
        .map_err(|_| anyhow::anyhow!(TimeoutError))?;
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    if !status.success() {
        let code = status
            .code()
            .map_or("signal".to_string(), |c| c.to_string());
        bail!("exited {code}: {}", argv.join(" "));
    }

    Ok(())
}

/// Run a command with stdin, stdout, and stderr all connected directly to the
/// terminal. Used for interactive programs (e.g. QEMU serial console).
/// Output is not captured — it goes straight to the terminal.
/// `timeout` kills the process if it runs longer than the given duration.
pub fn run_command_tty(argv: &[String], dir: Option<&str>, env: &HashMap<String, String>, timeout: Option<Duration>) -> Result<()> {
    let (program, rest) = match argv {
        [] => return Ok(()),
        [program, rest @ ..] => (program, rest),
    };

    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    for (k, v) in env {
        cmd.env(k, v);
    }

    if let Some(d) = dir {
        cmd.current_dir(d);
    }

    let mut child = cmd.spawn()?;
    let status = wait_with_timeout(&mut child, timeout)
        .map_err(|_| anyhow::anyhow!(TimeoutError))?;

    if !status.success() {
        let code = status
            .code()
            .map_or("signal".to_string(), |c| c.to_string());
        bail!("exited {code}: {}", argv.join(" "));
    }

    Ok(())
}
