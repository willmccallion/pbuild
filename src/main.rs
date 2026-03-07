use std::process::ExitCode;

use anyhow::{Context, Result};
use pbuild::{
    config::{load_build_file, resolve_target, to_rules},
    engine::{execute_plan, Config},
    graph::build_plan,
};

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1).peekable();

    let mut jobs: usize = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4);
    let mut dry_run = false;
    let mut verbose = false;
    let mut target_arg: Option<String> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" | "--dry-run" => dry_run = true,
            "-v" | "--verbose" => verbose = true,
            "-j" | "--jobs" => {
                let val = args.next().ok_or_else(|| anyhow::anyhow!("-j requires a value"))?;
                jobs = val.parse().context("-j requires a positive integer")?;
            }
            a if a.starts_with("-j") => {
                jobs = a[2..].parse().context("-j requires a positive integer")?;
            }
            _ => target_arg = Some(arg),
        }
    }

    let bf = load_build_file()?;
    let rules = to_rules(&bf)?;
    let root = resolve_target(&bf, target_arg.as_deref())?;
    let plan = build_plan(&rules, &root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let cfg = Config { jobs, dry_run, verbose };
    execute_plan(&cfg, &plan)?;

    Ok(())
}

fn main() -> ExitCode {
    if let Err(e) = run() {
        eprintln!("pbuild: {e}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
