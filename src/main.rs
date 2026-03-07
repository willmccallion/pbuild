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
    trust: bool,
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
      --trust          Skip safety checks for dangerous commands (sudo, rm -rf, etc.)

Special targets:
  init                 Write a starter pbuild.toml in the current directory
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
            "--trust" => args.trust = true,
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

fn cmd_init() -> Result<()> {
    if std::path::Path::new("pbuild.toml").exists() {
        anyhow::bail!("pbuild.toml already exists");
    }
    fs::write("pbuild.toml", INIT_TEMPLATE).context("failed to write pbuild.toml")?;
    println!("wrote pbuild.toml");
    Ok(())
}

const INIT_TEMPLATE: &str = r#"# pbuild.toml — https://github.com/yourname/pbuild-rs
#
# Run `pbuild --list` to see all targets.
# Run `pbuild <target>` to build a specific target.
# Run `pbuild` to build the default target.

[config]
default = "build"           # target to build when none is specified
# jobs  = 4                 # max parallel rules (default: logical CPUs)
# env   = ["CC", "CFLAGS"]  # env vars that trigger a full rebuild when changed

# [vars]
# Define reusable values with {{name}} interpolation in commands.
# cargo   = "cargo"
# python  = ".venv/bin/python"

# ── Rules ────────────────────────────────────────────────────────────────────
#
# Each rule is a TOML table.  Minimal required field: `command` or `commands`.
#
# Fields:
#   type        = "task" | "file"   (default: file)
#   command     = ["cmd", "arg"]    single command
#   commands    = [["cmd1"], ...]   multiple sequential commands
#   shell       = true              run via sh -c (enables pipes, globs, &&)
#   inputs      = ["src/**/*.rs"]   files to hash for dirty-checking (globs ok)
#   output      = "app"             file produced; hashed after success
#   deps        = ["other-rule"]    rules that must build first
#   depfile     = "main.d"          compiler depfile; pbuild injects -MF
#   description = "..."             shown in `pbuild --list`
#   group       = "Build"           group heading in `pbuild --list`

[build]
group       = "Build"
description = "Build the project"
type        = "task"
command     = ["echo", "replace me with your build command"]

[test]
group       = "Build"
description = "Run tests"
type        = "task"
command     = ["echo", "replace me with your test command"]

[clean]
description = "Remove build artifacts"
type        = "task"
command     = ["echo", "replace me with your clean command"]
"#;

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

/// Programs whose presence as the first token of a command is flagged.
const DANGEROUS_PROGRAMS: &[&str] = &[
    "sudo", "su", "doas", "pkexec",              // privilege escalation
    "chmod", "chown", "chgrp",                   // permission changes
    "dd", "mkfs", "fdisk", "parted",             // disk operations
    "passwd", "useradd", "userdel", "usermod",   // user management
    "iptables", "ip6tables", "nft",              // firewall changes
    "mount", "umount",                           // filesystem mounting
    "systemctl", "service",                      // system service control
    "crontab",                                   // scheduled task modification
    "at",                                        // one-off scheduled commands
    "install",                                   // copies files + sets permissions/owner
];

/// Shell command fragments that are flagged when `shell = true`.
const DANGEROUS_SHELL_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "rm -f /",
    "> /dev/",
    "> /etc/",
    "| sh",
    "| bash",
    "| zsh",
    "| sudo",
    "eval ",
    ":(){:|:&};:",  // fork bomb
];

/// Argument prefixes that indicate a system path destination.
/// Checked on non-shell commands where argv is unambiguous.
const DANGEROUS_PATH_PREFIXES: &[&str] = &[
    "/etc/", "/usr/", "/bin/", "/sbin/",
    "/boot/", "/sys/", "/proc/", "/lib/", "/lib64/",
];

/// Check every rule's commands for dangerous patterns.
/// Returns a list of human-readable warnings (one per offending command).
fn safety_warnings(rules: &[pbuild::types::Rule]) -> Vec<String> {
    let mut warnings = Vec::new();

    for rule in rules {
        let name = &rule.target;
        for cmd in &rule.commands {
            // Check first token against the dangerous programs list.
            if let Some(program) = cmd.first() {
                let prog = std::path::Path::new(program)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(program);
                if DANGEROUS_PROGRAMS.contains(&prog) {
                    warnings.push(format!(
                        "rule `{name}` runs `{prog}` which requires elevated privileges or modifies system state"
                    ));
                }
            }

            // For non-shell commands, check arguments for system path destinations.
            // We skip shell commands since we can't reliably parse them.
            if !rule.shell {
                for arg in cmd.iter().skip(1) {
                    if DANGEROUS_PATH_PREFIXES.iter().any(|p| arg.starts_with(p)) {
                        warnings.push(format!(
                            "rule `{name}` writes to system path `{arg}`"
                        ));
                        break; // one warning per command is enough
                    }
                }
            }

            // For shell rules, scan the joined command string for dangerous patterns.
            if rule.shell {
                let joined = cmd.join(" ");
                for pattern in DANGEROUS_SHELL_PATTERNS {
                    if joined.contains(pattern) {
                        warnings.push(format!(
                            "rule `{name}` contains `{pattern}` in a shell command"
                        ));
                    }
                }
            }
        }
    }

    warnings
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

    if args.target.as_deref() == Some("init") {
        return cmd_init();
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

    if !args.trust && !bf.config.trust {
        let warnings = safety_warnings(&rules);
        if !warnings.is_empty() {
            for w in &warnings {
                eprintln!("pbuild: unsafe: {w}");
            }
            eprintln!("pbuild: refusing to run. review the commands and pass --trust to proceed.");
            return Err(anyhow::anyhow!("unsafe commands detected"));
        }
    }

    let root = resolve_target(&bf, args.target.as_deref())?;
    let plan = build_plan(&rules, &root).map_err(|e| anyhow::anyhow!("{e}"))?;

    let cfg = Config {
        jobs,
        dry_run: args.dry_run,
        verbose: args.verbose,
        keep_going: args.keep_going,
        env: bf.config.env.clone(),
        ui: bf.ui.clone(),
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
