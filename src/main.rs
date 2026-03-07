use std::process::ExitCode;

use anyhow::{Context, Result};
use pbuild::{
    config::{load_build_file, resolve_target, to_rules},
    engine::{execute_plan, Config},
    graph::build_plan,
};

struct Args {
    jobs: usize,
    dry_run: bool,
    verbose: bool,
    list: bool,
    target: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            jobs: std::thread::available_parallelism()
                .map(std::num::NonZero::get)
                .unwrap_or(4),
            dry_run: false,
            verbose: false,
            list: false,
            target: None,
        }
    }
}

fn parse_args() -> Result<Args> {
    let mut raw = std::env::args().skip(1).peekable();
    let mut args = Args::default();

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "-n" | "--dry-run" => args.dry_run = true,
            "-v" | "--verbose" => args.verbose = true,
            "-l" | "--list"   => args.list = true,
            "-j" | "--jobs"   => {
                let val = raw.next().ok_or_else(|| anyhow::anyhow!("-j requires a value"))?;
                args.jobs = val.parse().context("-j requires a positive integer")?;
            }
            a if a.starts_with("-j") => {
                args.jobs = a[2..].parse().context("-j requires a positive integer")?;
            }
            _ => args.target = Some(arg),
        }
    }

    Ok(args)
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let bf = load_build_file()?;

    if args.list {
        let mut names: Vec<&str> = bf.rules.keys().map(String::as_str).collect();
        names.sort_unstable();
        for name in names {
            if bf.default.as_deref() == Some(name) {
                println!("{name} (default)");
            } else {
                println!("{name}");
            }
        }
        return Ok(());
    }

    let rules = to_rules(&bf)?;
    let root = resolve_target(&bf, args.target.as_deref())?;
    let plan = build_plan(&rules, &root)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let cfg = Config { jobs: args.jobs, dry_run: args.dry_run, verbose: args.verbose };
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
