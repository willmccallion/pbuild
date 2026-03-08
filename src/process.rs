use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};

use anyhow::{Result, bail};

/// Run a command and capture its stdout+stderr combined into a single buffer.
/// Returns the captured bytes and an error if the command exits non-zero.
/// `dir` sets the working directory; `None` inherits the current directory.
/// `env` sets extra environment variables for this invocation only.
pub fn run_command(argv: &[String], dir: Option<&str>, env: &HashMap<String, String>) -> Result<Vec<u8>> {
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

    let Output {
        status,
        stdout,
        stderr,
    } = cmd.output()?;

    // Interleave isn't possible post-hoc, so emit stderr after stdout.
    // In practice most tools write to stderr for errors and stdout for output,
    // so this ordering is fine for atomic buffered display.
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
pub fn run_command_streaming(argv: &[String], dir: Option<&str>, env: &HashMap<String, String>) -> Result<()> {
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

    let status = child.wait()?;
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
