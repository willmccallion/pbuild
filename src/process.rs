use std::process::{Command, Output, Stdio};

use anyhow::{Result, bail};

/// Run a command and capture its stdout+stderr combined into a single buffer.
/// Returns the captured bytes and an error if the command exits non-zero.
/// `dir` sets the working directory; `None` inherits the current directory.
pub fn run_command(argv: &[String], dir: Option<&str>) -> Result<Vec<u8>> {
    let (program, rest) = match argv {
        [] => return Ok(Vec::new()),
        [program, rest @ ..] => (program, rest),
    };

    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(d) = dir {
        cmd.current_dir(d);
    }

    let Output { status, stdout, stderr } = cmd.output()?;

    // Interleave isn't possible post-hoc, so emit stderr after stdout.
    // In practice most tools write to stderr for errors and stdout for output,
    // so this ordering is fine for atomic buffered display.
    let mut combined = stdout;
    combined.extend_from_slice(&stderr);

    if !status.success() {
        let code = status
            .code()
            .map_or("signal".to_string(), |c| c.to_string());
        bail!("exited {code}: {}\n{}", argv.join(" "), String::from_utf8_lossy(&combined));
    }

    Ok(combined)
}
