use std::fs;
use std::process::ExitCode;

use anyhow::{Context, Result};
use pbuild::{
    config::{expand_inputs, load_build_file, resolve_target, to_rules},
    engine::{execute_plan, Config},
    graph::build_plan,
    hash,
};

#[allow(clippy::struct_excessive_bools)]
struct Args {
    jobs: usize,
    dry_run: bool,
    verbose: bool,
    keep_going: bool,
    list: bool,
    help: bool,
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
            keep_going: false,
            list: false,
            help: false,
            target: None,
        }
    }
}

fn print_help() {
    println!("\
Usage: pbuild [OPTIONS] [TARGET]
       pbuild clean
       pbuild why <TARGET>

Options:
  -j <N>, --jobs <N>   Run at most N rules in parallel (default: logical CPUs)
  -n, --dry-run        Print commands without running them
  -k, --keep-going     Keep building independent rules after a failure
  -v, --verbose        Print skipped rules
  -l, --list           List all available targets and exit
  -h, --help           Print this help and exit

Special targets:
  clean                Delete all rule outputs and .pbuild.lock
  why <TARGET>         Explain why a target would rebuild");
}

fn parse_args() -> Result<Args> {
    let mut raw = std::env::args().skip(1).peekable();
    let mut args = Args::default();

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "-n" | "--dry-run"     => args.dry_run = true,
            "-v" | "--verbose"     => args.verbose = true,
            "-k" | "--keep-going"  => args.keep_going = true,
            "-l" | "--list"        => args.list = true,
            "-h" | "--help"        => args.help = true,
            "-j" | "--jobs"        => {
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

fn remove_if_exists(path: &str) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => { println!("rm {path}"); Ok(()) }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to remove {path}")),
    }
}

fn cmd_why(target_name: &str) -> Result<()> {
    let bf = load_build_file()?;
    let raw = bf.rules.get(target_name)
        .ok_or_else(|| anyhow::anyhow!("no rule for target: {target_name}"))?;

    let lf = hash::read_lock_file().context("could not read .pbuild.lock")?;
    let inputs = expand_inputs(&raw.inputs)?;

    println!("target: {target_name}");

    if inputs.is_empty() {
        println!("  no inputs declared — always runs");
    } else {
        println!("  inputs:");
        for path in &inputs {
            let status = match hash::is_dirty(&lf, path) {
                Ok(true)  => "CHANGED",
                Ok(false) => "clean",
                Err(e)    => return Err(e).with_context(|| format!("could not hash {path}")),
            };
            println!("    {path}  {status}");
        }
    }

    if !raw.deps.is_empty() {
        println!("  deps:");
        for dep in &raw.deps {
            let built = bf.rules.get(dep.as_str())
                .and_then(|r| if r.output.is_empty() { None } else { Some(r.output.as_str()) })
                .is_some_and(|out| lf.contains_key(out));
            let status = if built { "built" } else { "never built" };
            println!("    {dep}  {status}");
        }
    }

    Ok(())
}

fn cmd_clean() -> Result<()> {
    // Build file is optional — if absent we can still wipe the lock file.
    if let Ok(bf) = load_build_file() {
        for raw in bf.rules.values() {
            if !raw.output.is_empty() {
                remove_if_exists(&raw.output)?;
            }
        }
    }

    remove_if_exists(".pbuild.lock")
}

fn run() -> Result<()> {
    // Detect `why` before full arg parsing — it takes its own positional argument.
    let raw_argv: Vec<String> = std::env::args().skip(1).collect();
    if raw_argv.first().map(String::as_str) == Some("why") {
        let target = raw_argv.get(1)
            .ok_or_else(|| anyhow::anyhow!("usage: pbuild why <target>"))?;
        return cmd_why(target);
    }

    let args = parse_args()?;

    if args.help {
        print_help();
        return Ok(());
    }

    if args.target.as_deref() == Some("clean") {
        return cmd_clean();
    }

    let bf = load_build_file()?;

    if args.list {
        let mut names: Vec<&str> = bf.rules.keys().map(String::as_str).collect();
        names.sort_unstable();
        for name in names {
            if bf.config.default.as_deref() == Some(name) {
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

    let cfg = Config { jobs: args.jobs, dry_run: args.dry_run, verbose: args.verbose, keep_going: args.keep_going };
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
