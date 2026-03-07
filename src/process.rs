use std::process::{Command, Stdio};

use anyhow::{Result, bail};

/// Run a command given as an argv list. Streams stdout/stderr to the terminal.
/// Returns an error if the command exits non-zero or cannot be spawned.
pub fn run_command(argv: &[String]) -> Result<()> {
    let (program, rest) = match argv {
        [] => return Ok(()),
        [program, rest @ ..] => (program, rest),
    };

    let status = Command::new(program)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        let code = status.code().map_or("signal".to_string(), |c| c.to_string());
        bail!("exited {code}: {}", argv.join(" "));
    }

    Ok(())
}
