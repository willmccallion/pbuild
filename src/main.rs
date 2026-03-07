use std::fs;
use std::process::ExitCode;

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use pbuild::{
    config::{BuildFile, expand_inputs, load_build_file, resolve_target, to_rules},
    engine::{Config, execute_plan},
    graph::build_plan,
    hash,
};

#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
struct Args {
    /// Explicitly set via `-j`; `None` means "not set, defer to config or default".
    jobs: Option<usize>,
    dry_run: bool,
    verbose: bool,
    keep_going: bool,
    list: bool,
    help: bool,
    target: Option<String>,
}

fn print_help() {
    println!(
        "\
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
  why <TARGET>         Explain why a target would rebuild"
    );
}

fn parse_args() -> Result<Args> {
    let mut raw = std::env::args().skip(1).peekable();
    let mut args = Args::default();

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "-n" | "--dry-run" => args.dry_run = true,
            "-v" | "--verbose" => args.verbose = true,
            "-k" | "--keep-going" => args.keep_going = true,
            "-l" | "--list" => args.list = true,
            "-h" | "--help" => args.help = true,
            "-j" | "--jobs" => {
                let val = raw
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("-j requires a value"))?;
                args.jobs = Some(val.parse().context("-j requires a positive integer")?);
            }
            a if a.starts_with("-j") => {
                args.jobs = Some(a[2..].parse().context("-j requires a positive integer")?);
            }
            _ => args.target = Some(arg),
        }
    }

    Ok(args)
}

fn remove_if_exists(path: &str) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {
            println!("rm {path}");
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to remove {path}")),
    }
}

fn cmd_why(target_name: &str) -> Result<()> {
    let bf = load_build_file()?;
    let raw = bf
        .rules
        .get(target_name)
        .ok_or_else(|| anyhow::anyhow!("no rule for target: {target_name}"))?;

    let lf = hash::read_lock_file().context("could not read .pbuild.lock")?;
    let inputs = expand_inputs(&raw.inputs)?;

    println!("target: {target_name}");

    // Merge declared inputs with depfile-discovered inputs from the lock file.
    let discovered = raw
        .depfile
        .as_deref()
        .map(|_| hash::load_depfile_inputs(&lf, &raw.output))
        .unwrap_or_default();

    if inputs.is_empty() && discovered.is_empty() {
        println!("  no inputs declared — always runs");
    } else {
        println!("  inputs:");
        for path in &inputs {
            let status = match hash::is_dirty(&lf, path) {
                Ok(true) => "CHANGED",
                Ok(false) => "clean",
                Err(e) => return Err(e).with_context(|| format!("could not hash {path}")),
            };
            println!("    {path}  {status}");
        }
        if !discovered.is_empty() {
            println!("  depfile inputs (auto-discovered):");
            for path in &discovered {
                let status = match hash::is_dirty(&lf, path) {
                    Ok(true) => "CHANGED",
                    Ok(false) => "clean",
                    Err(e) => return Err(e).with_context(|| format!("could not hash {path}")),
                };
                println!("    {path}  {status}");
            }
        }
    }

    if !raw.deps.is_empty() {
        println!("  deps:");
        for dep in &raw.deps {
            let built = bf
                .rules
                .get(dep.as_str())
                .and_then(|r| {
                    if r.output.is_empty() {
                        None
                    } else {
                        Some(r.output.as_str())
                    }
                })
                .is_some_and(|out| lf.contains_key(out));
            let status = if built { "built" } else { "never built" };
            println!("    {dep}  {status}");
        }
    }

    if !bf.config.env.is_empty() {
        println!("  env:");
        for var in &bf.config.env {
            let current = std::env::var(var).ok();
            let stored = hash::env_stored_value(&lf, var);
            let dirty = current.as_deref() != stored;

            let current_display = match &current {
                Some(v) => format!("{var}={v}"),
                None => format!("{var} (unset)"),
            };

            if dirty {
                let was = match stored {
                    Some(v) => format!("was: {v}"),
                    None => "was: unset".to_string(),
                };
                println!("    {current_display}  CHANGED ({was})");
            } else {
                println!("    {current_display}  clean");
            }
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

fn print_list(bf: &BuildFile) {
    // Collect entries, sorted by name within each group.
    // Group key: explicit group name, or "" (shown last as ungrouped).
    let mut groups: BTreeMap<&str, Vec<(&str, &pbuild::config::RawRule)>> = BTreeMap::new();
    let mut sorted: Vec<(&str, &pbuild::config::RawRule)> =
        bf.rules.iter().map(|(k, v)| (k.as_str(), v)).collect();
    sorted.sort_by_key(|(name, _)| *name);

    for (name, raw) in &sorted {
        let group = raw.group.as_deref().unwrap_or("");
        groups.entry(group).or_default().push((name, raw));
    }

    // Compute column width for alignment across all entries.
    let col_width = sorted.iter().map(|(n, _)| n.len()).max().unwrap_or(0) + 2;

    // Print ungrouped first (empty key sorts first in BTreeMap), then named groups.
    // We want named groups first, ungrouped last — collect and reorder.
    let mut ungrouped = None;
    let mut named: Vec<(&str, &Vec<(&str, &pbuild::config::RawRule)>)> = Vec::new();
    for (group, entries) in &groups {
        if group.is_empty() {
            ungrouped = Some(entries);
        } else {
            named.push((group, entries));
        }
    }

    let print_entries = |entries: &Vec<(&str, &pbuild::config::RawRule)>| {
        for (name, raw) in entries {
            let is_default = bf.config.default.as_deref() == Some(name);
            let suffix = if is_default { "  (default)" } else { "" };
            if let Some(desc) = &raw.description {
                println!("  {name:<col_width$}{desc}{suffix}");
            } else {
                println!("  {name}{suffix}");
            }
        }
    };

    for (group, entries) in &named {
        println!("{group}");
        print_entries(entries);
    }
    if let Some(entries) = ungrouped {
        if !named.is_empty() {
            println!("Other");
        }
        print_entries(entries);
    }
}

fn run() -> Result<()> {
    // Detect `why` before full arg parsing — it takes its own positional argument.
    let raw_argv: Vec<String> = std::env::args().skip(1).collect();
    if raw_argv.first().map(String::as_str) == Some("why") {
        let target = raw_argv
            .get(1)
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
        print_list(&bf);
        return Ok(());
    }

    let jobs = args.jobs.or(bf.config.jobs).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4)
    });

    let rules = to_rules(&bf)?;
    let root = resolve_target(&bf, args.target.as_deref())?;
    let plan = build_plan(&rules, &root).map_err(|e| anyhow::anyhow!("{e}"))?;

    let cfg = Config {
        jobs,
        dry_run: args.dry_run,
        verbose: args.verbose,
        keep_going: args.keep_going,
        env: bf.config.env.clone(),
    };
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
