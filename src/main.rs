use std::fs;
use std::io::IsTerminal as _;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use pbuild::{
    config::{BuildFile, expand_inputs, load_build_file, resolve_target, to_rules},
    engine::{Config, check_status, execute_plan},
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
    only: bool,
    watch: bool,
    completion: Option<String>,
    log: Option<String>,
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
      --only           Build just the named target without running its dependencies
      --log <file>     Tee pbuild's output lines to a file (appends; no ANSI codes)
  -w, --watch          Rebuild automatically when input files change
      --completion     Print shell completion script (fish, bash, or zsh)

Special targets:
  init                 Write a starter pbuild.toml in the current directory
  add <name>           Interactively scaffold a new rule in pbuild.toml
  status [TARGET]      Show which targets are dirty (would rebuild)
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
            "--only" => args.only = true,
            "-w" | "--watch" => args.watch = true,
            "--completion" => {
                let val = raw
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--completion requires a shell (fish, bash, zsh)"))?;
                args.completion = Some(val);
            }
            "--log" => {
                let val = raw
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--log requires a file path"))?;
                args.log = Some(val);
            }
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

fn prompt(question: &str, default: &str) -> Result<String> {
    use std::io::Write as _;
    if default.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{default}]: ");
    }
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed)
    }
}

fn cmd_completion(shell: &str) -> Result<()> {
    match shell {
        "fish" => print!("{}", COMPLETION_FISH),
        "bash" => print!("{}", COMPLETION_BASH),
        "zsh"  => print!("{}", COMPLETION_ZSH),
        other  => anyhow::bail!("unknown shell `{other}` — supported: fish, bash, zsh"),
    }
    Ok(())
}

const COMPLETION_FISH: &str = r#"# pbuild fish completion
# Install: pbuild --completion fish > ~/.config/fish/completions/pbuild.fish

function __pbuild_targets
    if test -f pbuild.toml
        pbuild --list 2>/dev/null | string match -r '^\s+\S+' | string trim | string match -r '^\S+'
    end
end

complete -c pbuild -f

# Flags
complete -c pbuild -s j -l jobs        -d 'Max parallel rules' -x
complete -c pbuild -s n -l dry-run     -d 'Print commands without running'
complete -c pbuild -s k -l keep-going  -d 'Keep building after a failure'
complete -c pbuild -s v -l verbose     -d 'Print skipped rules'
complete -c pbuild -s l -l list        -d 'List all targets'
complete -c pbuild -s h -l help        -d 'Print help'
complete -c pbuild -s w -l watch       -d 'Rebuild on file changes'
complete -c pbuild -l trust            -d 'Skip safety checks'
complete -c pbuild -l only             -d 'Build target without its deps'
complete -c pbuild -l log              -d 'Tee output to a file' -r
complete -c pbuild -l completion       -d 'Print completion script' -x -a 'fish bash zsh'

# Special subcommands
complete -c pbuild -n '__fish_is_first_arg' -a 'init'   -d 'Write starter pbuild.toml'
complete -c pbuild -n '__fish_is_first_arg' -a 'add'    -d 'Add a new rule'
complete -c pbuild -n '__fish_is_first_arg' -a 'status' -d 'Show dirty/clean state'
complete -c pbuild -n '__fish_is_first_arg' -a 'clean'  -d 'Delete outputs and lock file'
complete -c pbuild -n '__fish_is_first_arg' -a 'why'    -d 'Explain why a target rebuilds'

# Targets from pbuild.toml
complete -c pbuild -n '__fish_is_first_arg' -a '(__pbuild_targets)'
"#;

const COMPLETION_BASH: &str = r#"# pbuild bash completion
# Install: eval "$(pbuild --completion bash)"

_pbuild_complete() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local prev="${COMP_WORDS[COMP_CWORD-1]}"

    case "$prev" in
        -j|--jobs) return ;;
        --log)     COMPREPLY=($(compgen -f -- "$cur")); return ;;
        --completion) COMPREPLY=($(compgen -W "fish bash zsh" -- "$cur")); return ;;
    esac

    if [[ "$cur" == -* ]]; then
        COMPREPLY=($(compgen -W "
            -j --jobs -n --dry-run -k --keep-going -v --verbose
            -l --list -h --help -w --watch
            --trust --only --log --completion
        " -- "$cur"))
        return
    fi

    local targets=""
    if [[ -f pbuild.toml ]]; then
        targets=$(pbuild --list 2>/dev/null | grep -oP '^\s+\K\S+')
    fi
    COMPREPLY=($(compgen -W "init add status clean why $targets" -- "$cur"))
}

complete -F _pbuild_complete pbuild
"#;

const COMPLETION_ZSH: &str = r#"#compdef pbuild
# pbuild zsh completion
# Install: pbuild --completion zsh > "${fpath[1]}/_pbuild"

_pbuild() {
    local -a targets
    if [[ -f pbuild.toml ]]; then
        targets=(${(f)"$(pbuild --list 2>/dev/null | grep -oP '^\s+\K\S+')"})
    fi

    _arguments \
        '(-j --jobs)'{-j,--jobs}'[Max parallel rules]:jobs' \
        '(-n --dry-run)'{-n,--dry-run}'[Print commands without running]' \
        '(-k --keep-going)'{-k,--keep-going}'[Keep building after failure]' \
        '(-v --verbose)'{-v,--verbose}'[Print skipped rules]' \
        '(-l --list)'{-l,--list}'[List all targets]' \
        '(-h --help)'{-h,--help}'[Print help]' \
        '(-w --watch)'{-w,--watch}'[Rebuild on file changes]' \
        '--trust[Skip safety checks]' \
        '--only[Build target without its deps]' \
        '--log[Tee output to a file]:file:_files' \
        '--completion[Print completion script]:shell:(fish bash zsh)' \
        ':target:(init add status clean why '"${targets[@]}"')'
}

_pbuild
"#;

fn cmd_watch(args: &Args) -> Result<()> {
    use notify::{Event, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::Duration;

    let bf = load_build_file()?;
    let jobs = args.jobs.or(bf.config.jobs).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4)
    });
    let rules = pbuild::config::to_rules(&bf)?;
    let root = pbuild::config::resolve_target(&bf, args.target.as_deref())?;
    let plan = pbuild::graph::build_plan(&rules, &root).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Collect all input files across all rules in the plan.
    let watch_paths: Vec<std::path::PathBuf> = plan
        .iter()
        .flat_map(|r| r.inputs.iter())
        .map(std::path::PathBuf::from)
        .chain(std::iter::once(std::path::PathBuf::from("pbuild.toml")))
        .collect();

    let cfg = pbuild::engine::Config {
        jobs,
        dry_run: args.dry_run,
        verbose: args.verbose,
        keep_going: args.keep_going,
        env: bf.config.env.clone(),
        ui: bf.ui.clone(),
    };

    let run_build = |cfg: &pbuild::engine::Config, plan: &[pbuild::types::Rule]| {
        println!("\x1b[2m──────────────────────────────\x1b[0m");
        if let Err(e) = pbuild::engine::execute_plan(cfg, plan) {
            eprintln!("pbuild: {e}");
        }
    };

    // Initial build.
    run_build(&cfg, &plan);

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(tx)?;

    for path in &watch_paths {
        // Watch the parent directory so renames and new files are caught.
        let watch_target = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path.as_path()
        };
        // Ignore errors for paths that don't exist yet.
        let _ = watcher.watch(watch_target, RecursiveMode::NonRecursive);
    }
    // Always watch the current directory for pbuild.toml changes.
    watcher.watch(std::path::Path::new("."), RecursiveMode::NonRecursive)?;

    println!("\x1b[2mWatching for changes… (Ctrl-C to stop)\x1b[0m");

    // Debounce: collect events for 50ms before triggering a rebuild.
    loop {
        // Block until at least one event arrives.
        let _ = rx.recv();
        // Drain any additional events that arrive within the debounce window.
        std::thread::sleep(Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        // Reload plan in case pbuild.toml changed.
        let bf = match load_build_file() {
            Ok(bf) => bf,
            Err(e) => { eprintln!("pbuild: {e}"); continue; }
        };
        let rules = match pbuild::config::to_rules(&bf) {
            Ok(r) => r,
            Err(e) => { eprintln!("pbuild: {e}"); continue; }
        };
        let root = match pbuild::config::resolve_target(&bf, args.target.as_deref()) {
            Ok(r) => r,
            Err(e) => { eprintln!("pbuild: {e}"); continue; }
        };
        let plan = match pbuild::graph::build_plan(&rules, &root) {
            Ok(p) => p,
            Err(e) => { eprintln!("pbuild: {e}"); continue; }
        };
        let cfg = pbuild::engine::Config {
            jobs,
            dry_run: args.dry_run,
            verbose: args.verbose,
            keep_going: args.keep_going,
            env: bf.config.env.clone(),
            ui: bf.ui.clone(),
        };
        run_build(&cfg, &plan);
    }
}

fn cmd_add(name: &str) -> Result<()> {
    if !std::path::Path::new("pbuild.toml").exists() {
        anyhow::bail!("no pbuild.toml found — run `pbuild init` first");
    }

    // Check for duplicate.
    let existing = fs::read_to_string("pbuild.toml").context("could not read pbuild.toml")?;
    let header = format!("[{name}]");
    let quoted_header = format!("[\"{name}\"]");
    if existing.contains(&header) || existing.contains(&quoted_header) {
        anyhow::bail!("rule `{name}` already exists in pbuild.toml");
    }

    println!("Adding rule `{name}` to pbuild.toml");
    println!("Press Enter to accept defaults.\n");

    let rule_type = prompt("type (task/file)", "task")?;
    let description = prompt("description", "")?;
    let group = prompt("group", "")?;
    let command_str = prompt("command", "")?;

    if command_str.is_empty() {
        anyhow::bail!("command is required");
    }

    // Split the command string into an argv array for TOML.
    let argv: Vec<String> = command_str
        .split_whitespace()
        .map(ToString::to_string)
        .collect();
    let argv_toml = argv
        .iter()
        .map(|a| format!("\"{}\"", a.replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");

    // Build the TOML snippet.
    let key = if name.contains('.') || name.contains(' ') || name.contains('/') {
        format!("[\"{name}\"]")
    } else {
        format!("[{name}]")
    };

    let mut snippet = format!("\n{key}\n");
    if !group.is_empty() {
        snippet.push_str(&format!("group       = \"{group}\"\n"));
    }
    if !description.is_empty() {
        snippet.push_str(&format!("description = \"{description}\"\n"));
    }
    if rule_type == "task" {
        snippet.push_str("type        = \"task\"\n");
    }
    snippet.push_str(&format!("command     = [{argv_toml}]\n"));

    let mut content = existing;
    // Ensure a trailing newline before appending.
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&snippet);

    fs::write("pbuild.toml", &content).context("failed to write pbuild.toml")?;
    println!("\nAdded `{name}` to pbuild.toml.");
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

fn cmd_status(target: Option<&str>) -> Result<()> {
    let bf = load_build_file()?;
    let rules = to_rules(&bf)?;
    let root = resolve_target(&bf, target)?;
    let plan = build_plan(&rules, &root).map_err(|e| anyhow::anyhow!("{e}"))?;
    let statuses = check_status(&plan)?;

    let col = statuses.iter().map(|(n, _)| n.len()).max().unwrap_or(0) + 2;
    for (name, dirty) in &statuses {
        if *dirty {
            println!("  {name:<col$} dirty");
        } else {
            println!("  {name:<col$} clean");
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
    // Try to load dirty state — silently skip if no lock file yet.
    let dirty_map: std::collections::HashMap<String, bool> = {
        let rules = pbuild::config::to_rules(bf).unwrap_or_default();
        check_status(&rules)
            .unwrap_or_default()
            .into_iter()
            .collect()
    };

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

    let is_tty = std::io::stdout().is_terminal();
    let print_entries = |entries: &Vec<(&str, &pbuild::config::RawRule)>| {
        for (name, raw) in entries {
            let is_default = bf.config.default.as_deref() == Some(name);
            let default_marker = if is_default { "  (default)" } else { "" };
            // Only show dirty marker when stdout is a TTY — piped output (e.g.
            // shell completions) must see clean target names only.
            let state = if is_tty {
                match dirty_map.get(*name) {
                    Some(true) => "  \x1b[33m*\x1b[0m",
                    _          => "   ",
                }
            } else {
                " "
            };
            if let Some(desc) = &raw.description {
                println!("{state}  {name:<col_width$}{desc}{default_marker}");
            } else {
                println!("{state}  {name}{default_marker}");
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
    "sudo",
    "su",
    "doas",
    "pkexec", // privilege escalation
    "chmod",
    "chown",
    "chgrp", // permission changes
    "dd",
    "mkfs",
    "fdisk",
    "parted", // disk operations
    "passwd",
    "useradd",
    "userdel",
    "usermod", // user management
    "iptables",
    "ip6tables",
    "nft", // firewall changes
    "mount",
    "umount", // filesystem mounting
    "systemctl",
    "service", // system service control
    "crontab", // scheduled task modification
    "at",      // one-off scheduled commands
    "install", // copies files + sets permissions/owner
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
    ":(){:|:&};:", // fork bomb
];

/// Argument prefixes that indicate a system path destination.
/// Checked on non-shell commands where argv is unambiguous.
const DANGEROUS_PATH_PREFIXES: &[&str] = &[
    "/etc/", "/usr/", "/bin/", "/sbin/", "/boot/", "/sys/", "/proc/", "/lib/", "/lib64/",
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
                        warnings.push(format!("rule `{name}` writes to system path `{arg}`"));
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
    // Detect `why` and `status` before full arg parsing — they take their own positional argument.
    let raw_argv: Vec<String> = std::env::args().skip(1).collect();
    if raw_argv.first().map(String::as_str) == Some("why") {
        let target = raw_argv
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("usage: pbuild why <target>"))?;
        return cmd_why(target);
    }
    if raw_argv.first().map(String::as_str) == Some("status") {
        let target = raw_argv.get(1).map(String::as_str);
        return cmd_status(target);
    }
    if raw_argv.first().map(String::as_str) == Some("add") {
        let name = raw_argv
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("usage: pbuild add <name>"))?;
        return cmd_add(name);
    }
    // --completion doesn't need a pbuild.toml — detect it early.
    if let Some(pos) = raw_argv.iter().position(|a| a == "--completion") {
        let shell = raw_argv
            .get(pos + 1)
            .ok_or_else(|| anyhow::anyhow!("--completion requires a shell (fish, bash, zsh)"))?;
        return cmd_completion(shell);
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

    if args.watch {
        return cmd_watch(&args);
    }

    let root = resolve_target(&bf, args.target.as_deref())?;

    let log_file = args
        .log
        .as_deref()
        .map(|path| {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("failed to open log file: {path}"))
        })
        .transpose()?;

    let ui = pbuild::ui::UiConfig {
        log: log_file.map(|f| Arc::new(Mutex::new(f))),
        ..bf.ui.clone()
    };

    let cfg = Config {
        jobs,
        dry_run: args.dry_run,
        verbose: args.verbose,
        keep_going: args.keep_going,
        env: bf.config.env.clone(),
        ui,
    };

    let plan = if args.only {
        // Run just the single target — no dependency resolution.
        // Clear deps so the wave scheduler doesn't wait for them.
        rules
            .into_iter()
            .find(|r| r.target == root)
            .map(|mut r| {
                r.deps.clear();
                vec![r]
            })
            .ok_or_else(|| anyhow::anyhow!("no rule for target: {root}"))?
    } else {
        build_plan(&rules, &root).map_err(|e| anyhow::anyhow!("{e}"))?
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
