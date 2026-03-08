use std::process::{Command, Stdio};

use anyhow::{Result, bail};

/// Run a command given as an argv list. Streams stdout/stderr to the terminal.
/// Returns an error if the command exits non-zero or cannot be spawned.
/// `dir` sets the working directory; `None` inherits the current directory.
pub fn run_command(argv: &[String], dir: Option<&str>) -> Result<()> {
    let (program, rest) = match argv {
        [] => return Ok(()),
        [program, rest @ ..] => (program, rest),
    };

    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if let Some(d) = dir {
        cmd.current_dir(d);
    }

    let status = cmd.status()?;

    if !status.success() {
        let code = status
            .code()
            .map_or("signal".to_string(), |c| c.to_string());
        bail!("exited {code}: {}", argv.join(" "));
    }

    Ok(())
}
