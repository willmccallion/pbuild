use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::IsTerminal as _;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use pbuild::{
    config::{BuildFile, apply_profile, expand_inputs, load_build_file, resolve_target, to_rules},
    engine::{Config, check_status, execute_plan},
    graph::{build_plan, print_dot, print_graph},
    hash,
};

#[allow(clippy::struct_excessive_bools, clippy::struct_field_names)]
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
    detect: bool,
    completion: Option<String>,
    log: Option<String>,
    profile: Option<String>,
    target: Option<String>,
    /// Extra arguments passed after `--` on the command line.
    extra_args: Vec<String>,
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
  -p, --profile <name> Activate a named profile from [config.profiles.<name>]
  -w, --watch          Rebuild automatically when input files change
      --completion     Print shell completion script (fish, bash, or zsh)
  --                   Pass remaining arguments to the target command (or {{args}})

Special targets:
  init                 Write a starter pbuild.toml in the current directory
  init --detect        Auto-detect project type and scaffold real targets
  import [Makefile]    Convert a Makefile into pbuild.toml (default: Makefile)
  add <name>           Interactively scaffold a new rule in pbuild.toml
  edit [TARGET]        Open pbuild.toml in $EDITOR at the given target's rule
  run <TARGET>         Alias for pbuild <TARGET> (explicit subcommand form)
  status [TARGET]      Show which targets are dirty (would rebuild)
  clean                Delete all rule outputs and .pbuild.lock
  clean <TARGET>       Delete one target's output and its lock entries
  touch <TARGET>       Hash inputs/output now — mark target clean without building
  prune                Remove stale entries from .pbuild.lock
  why <TARGET>         Explain why a target would rebuild
  graph [TARGET]       Print the dependency graph for a target
  graph --dot [TARGET] Emit Graphviz DOT format"
    );
}

fn parse_args() -> Result<Args> {
    let mut raw = std::env::args().skip(1).peekable();
    let mut args = Args::default();

    // `pbuild run <target> [-- extra]` is an alias for `pbuild <target> [-- extra]`.
    if raw.peek().map(String::as_str) == Some("run") {
        raw.next(); // consume "run"
    }

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--" => {
                args.extra_args = raw.collect();
                break;
            }
            "-n" | "--dry-run" => args.dry_run = true,
            "-v" | "--verbose" => args.verbose = true,
            "-k" | "--keep-going" => args.keep_going = true,
            "-l" | "--list" => args.list = true,
            "-h" | "--help" => args.help = true,
            "--trust" => args.trust = true,
            "--only" => args.only = true,
            "-w" | "--watch" => args.watch = true,
            "--detect" => args.detect = true,
            "--completion" => {
                let val = raw.next().ok_or_else(|| {
                    anyhow::anyhow!("--completion requires a shell (fish, bash, zsh)")
                })?;
                args.completion = Some(val);
            }
            "--log" => {
                let val = raw
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--log requires a file path"))?;
                args.log = Some(val);
            }
            "--profile" | "-p" => {
                let val = raw
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--profile requires a name"))?;
                args.profile = Some(val);
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

fn cmd_why(target_name: &str, profile: Option<&str>) -> Result<()> {
    let mut bf = load_build_file()?;
    if let Some(p) = profile {
        apply_profile(&mut bf, p)?;
    }
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

    if !raw.env.is_empty() {
        println!("  rule env overrides:");
        let mut sorted: Vec<_> = raw.env.iter().collect();
        sorted.sort_by_key(|(k, _)| *k);
        for (k, v) in sorted {
            println!("    {k}={v}");
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
        "fish" => print!("{COMPLETION_FISH}"),
        "bash" => print!("{COMPLETION_BASH}"),
        "zsh" => print!("{COMPLETION_ZSH}"),
        other => anyhow::bail!("unknown shell `{other}` — supported: fish, bash, zsh"),
    }
    Ok(())
}

const COMPLETION_FISH: &str = r"# pbuild fish completion
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
complete -c pbuild -l detect           -d 'Auto-detect project type (use with init)'
complete -c pbuild -l log              -d 'Tee output to a file' -r
complete -c pbuild -s p -l profile    -d 'Activate a named profile' -x
complete -c pbuild -l completion       -d 'Print completion script' -x -a 'fish bash zsh'

# Special subcommands
complete -c pbuild -n '__fish_is_first_arg' -a 'init'   -d 'Write starter pbuild.toml'
complete -c pbuild -n '__fish_is_first_arg' -a 'import' -d 'Convert a Makefile to pbuild.toml'
complete -c pbuild -n '__fish_is_first_arg' -a 'add'    -d 'Add a new rule'
complete -c pbuild -n '__fish_is_first_arg' -a 'edit'   -d 'Open pbuild.toml at target in $EDITOR'
complete -c pbuild -n '__fish_is_first_arg' -a 'run'    -d 'Build a target (explicit subcommand)'
complete -c pbuild -n '__fish_is_first_arg' -a 'status' -d 'Show dirty/clean state'
complete -c pbuild -n '__fish_is_first_arg' -a 'clean'  -d 'Delete outputs and lock file'
complete -c pbuild -n '__fish_is_first_arg' -a 'touch'  -d 'Mark target clean without building'
complete -c pbuild -n '__fish_is_first_arg' -a 'prune'  -d 'Remove stale lock file entries'
complete -c pbuild -n '__fish_is_first_arg' -a 'why'    -d 'Explain why a target rebuilds'
complete -c pbuild -n '__fish_is_first_arg' -a 'graph'  -d 'Print dependency graph'

# Targets from pbuild.toml
complete -c pbuild -n '__fish_is_first_arg' -a '(__pbuild_targets)'
";

const COMPLETION_BASH: &str = r#"# pbuild bash completion
# Install: eval "$(pbuild --completion bash)"

_pbuild_complete() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local prev="${COMP_WORDS[COMP_CWORD-1]}"

    case "$prev" in
        -j|--jobs) return ;;
        --log)     COMPREPLY=($(compgen -f -- "$cur")); return ;;
        -p|--profile) return ;;
        --completion) COMPREPLY=($(compgen -W "fish bash zsh" -- "$cur")); return ;;
    esac

    if [[ "$cur" == -* ]]; then
        COMPREPLY=($(compgen -W "
            -j --jobs -n --dry-run -k --keep-going -v --verbose
            -l --list -h --help -w --watch -p --profile
            --trust --only --detect --log --completion
        " -- "$cur"))
        return
    fi

    local targets=""
    if [[ -f pbuild.toml ]]; then
        targets=$(pbuild --list 2>/dev/null | grep -oP '^\s+\K\S+')
    fi
    COMPREPLY=($(compgen -W "init import add edit run status clean touch prune why graph $targets" -- "$cur"))
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
        '--detect[Auto-detect project type (use with init)]' \
        '--log[Tee output to a file]:file:_files' \
        '(-p --profile)'{-p,--profile}'[Activate a named profile]:profile' \
        '--completion[Print completion script]:shell:(fish bash zsh)' \
        ':target:(init import add edit run status clean touch prune why graph '"${targets[@]}"')'
}

_pbuild
"#;

fn cmd_watch(args: &Args) -> Result<()> {
    use notify::{Event, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::Duration;

    let mut bf = load_build_file()?;
    if let Some(p) = &args.profile {
        apply_profile(&mut bf, p)?;
    }
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
        extra_args: Vec::new(),
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
        let mut bf = match load_build_file() {
            Ok(bf) => bf,
            Err(e) => {
                eprintln!("pbuild: {e}");
                continue;
            }
        };
        if let Some(p) = &args.profile {
            if let Err(e) = apply_profile(&mut bf, p) {
                eprintln!("pbuild: {e}");
                continue;
            }
        }
        let rules = match pbuild::config::to_rules(&bf) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("pbuild: {e}");
                continue;
            }
        };
        let root = match pbuild::config::resolve_target(&bf, args.target.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("pbuild: {e}");
                continue;
            }
        };
        let plan = match pbuild::graph::build_plan(&rules, &root) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("pbuild: {e}");
                continue;
            }
        };
        let cfg = pbuild::engine::Config {
            jobs,
            dry_run: args.dry_run,
            verbose: args.verbose,
            keep_going: args.keep_going,
            env: bf.config.env.clone(),
            ui: bf.ui.clone(),
            extra_args: Vec::new(),
        };
        run_build(&cfg, &plan);
    }
}

/// Guess a sensible default command for a well-known rule name given the project type.
fn suggest_command(name: &str) -> &'static str {
    let has_cargo = std::path::Path::new("Cargo.toml").exists();
    let has_npm = std::path::Path::new("package.json").exists();
    let has_python = std::path::Path::new("pyproject.toml").exists()
        || std::path::Path::new("setup.py").exists();
    let has_go = std::path::Path::new("go.mod").exists();

    match name {
        "build" => {
            if has_cargo { "cargo build" }
            else if has_npm { "npm run build" }
            else if has_python { "python -m build" }
            else if has_go { "go build ./..." }
            else { "" }
        }
        "test" | "tests" => {
            if has_cargo { "cargo test" }
            else if has_npm { "npm test" }
            else if has_python { "python -m pytest" }
            else if has_go { "go test ./..." }
            else { "" }
        }
        "lint" | "check" => {
            if has_cargo { "cargo clippy -- -D warnings" }
            else if has_npm { "npm run lint" }
            else if has_python { "ruff check ." }
            else if has_go { "golangci-lint run" }
            else { "" }
        }
        "fmt" | "format" => {
            if has_cargo { "cargo fmt --all" }
            else if has_npm { "npm run format" }
            else if has_python { "ruff format ." }
            else if has_go { "gofmt -w ." }
            else { "" }
        }
        "clean" => {
            if has_cargo { "cargo clean" }
            else if has_npm { "rm -rf node_modules dist" }
            else if has_go { "go clean ./..." }
            else { "" }
        }
        "run" | "serve" | "start" => {
            if has_cargo { "cargo run" }
            else if has_npm { "npm start" }
            else if has_python { "python -m app" }
            else if has_go { "go run ." }
            else { "" }
        }
        "release" => {
            if has_cargo { "cargo build --release" }
            else { "" }
        }
        "install" => {
            if has_cargo { "cargo install --path ." }
            else if has_npm { "npm install" }
            else { "" }
        }
        _ => "",
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
    let default_cmd = suggest_command(name);
    let command_str = prompt("command", default_cmd)?;

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
        let _ = writeln!(snippet, "group       = \"{group}\"");
    }
    if !description.is_empty() {
        let _ = writeln!(snippet, "description = \"{description}\"");
    }
    if rule_type == "task" {
        snippet.push_str("type        = \"task\"\n");
    }
    let _ = writeln!(snippet, "command     = [{argv_toml}]");

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

fn cmd_init(detect: bool) -> Result<()> {
    if std::path::Path::new("pbuild.toml").exists() {
        anyhow::bail!("pbuild.toml already exists");
    }
    let content = if detect {
        detect_template()
    } else {
        INIT_TEMPLATE.to_string()
    };
    fs::write("pbuild.toml", &content).context("failed to write pbuild.toml")?;
    println!("wrote pbuild.toml");
    Ok(())
}

/// Detect project type(s) in the current directory and generate a tailored template.
#[allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::format_push_string
)]
fn detect_template() -> String {
    let has_cargo = std::path::Path::new("Cargo.toml").exists();
    let has_npm = std::path::Path::new("package.json").exists();
    let has_python = std::path::Path::new("pyproject.toml").exists()
        || std::path::Path::new("setup.py").exists()
        || std::path::Path::new("setup.cfg").exists();
    let has_make = std::path::Path::new("Makefile").exists();
    let has_go = std::path::Path::new("go.mod").exists();
    let has_cmake = std::path::Path::new("CMakeLists.txt").exists();

    // Detect whether this is a Cargo workspace (has [workspace] in Cargo.toml).
    let is_workspace = has_cargo
        && fs::read_to_string("Cargo.toml")
            .map(|s| s.contains("[workspace]"))
            .unwrap_or(false);

    // Detect maturin (Python + Cargo = PyO3 project).
    let has_maturin = has_cargo && has_python;

    // Detect venv.
    let venv_python = if std::path::Path::new(".venv/bin/python").exists() {
        ".venv/bin/python"
    } else {
        "python3"
    };

    let detected: Vec<&str> = [
        has_cargo.then_some(if is_workspace {
            "Cargo workspace"
        } else {
            "Cargo"
        }),
        has_npm.then_some("npm"),
        has_python.then_some("Python"),
        has_go.then_some("Go"),
        has_cmake.then_some("CMake"),
        has_make.then_some("Makefile"),
    ]
    .into_iter()
    .flatten()
    .collect();

    let detected_str = if detected.is_empty() {
        String::new()
    } else {
        format!("# Detected: {}\n", detected.join(", "))
    };

    let mut out = format!(
        "# pbuild.toml\n\
         {detected_str}\
         \n\
         [config]\n\
         default = \"build\"\n\
         \n"
    );

    // ── Rust / Cargo ──────────────────────────────────────────────────────────
    if has_cargo && !has_maturin {
        let ws_flag = if is_workspace { " --workspace" } else { "" };
        out.push_str(&format!(
            "[vars]\n\
             cargo = \"cargo\"\n\
             \n\
             [build]\n\
             group       = \"Build\"\n\
             description = \"Build in release mode\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\", \"Cargo.lock\"]\n\
             command     = [\"{{{{cargo}}}}\", \"build\", \"--release\"{ws_flag}]\n\
             \n\
             [check]\n\
             group       = \"Build\"\n\
             description = \"Fast type-check\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\"]\n\
             command     = [\"{{{{cargo}}}}\", \"check\"{ws_flag}]\n\
             \n\
             [test]\n\
             group       = \"Build\"\n\
             description = \"Run tests\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\"]\n\
             command     = [\"{{{{cargo}}}}\", \"test\"{ws_flag}]\n\
             \n\
             [clippy]\n\
             group       = \"Quality\"\n\
             description = \"Run clippy linter\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\"]\n\
             command     = [\"{{{{cargo}}}}\", \"clippy\"{ws_flag}, \"--\", \"-D\", \"warnings\"]\n\
             \n\
             [fmt]\n\
             group       = \"Quality\"\n\
             description = \"Format code\"\n\
             type        = \"task\"\n\
             command     = [\"{{{{cargo}}}}\", \"fmt\"]\n\
             \n\
             [fmt-check]\n\
             group       = \"Quality\"\n\
             description = \"Check formatting\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\"]\n\
             command     = [\"{{{{cargo}}}}\", \"fmt\", \"--\", \"--check\"]\n\
             \n\
             [clean]\n\
             description = \"Remove build artifacts\"\n\
             type        = \"task\"\n\
             command     = [\"{{{{cargo}}}}\", \"clean\"]\n\
             "
        ));
    }

    // ── PyO3 / Maturin (Rust + Python) ────────────────────────────────────────
    if has_maturin {
        let ws_flag = if is_workspace { " --workspace" } else { "" };
        out.push_str(&format!(
            "[vars]\n\
             cargo  = \"cargo\"\n\
             python = \"{venv_python}\"\n\
             \n\
             [build]\n\
             group       = \"Build\"\n\
             description = \"Install Python extension (editable)\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\"]\n\
             command     = [\"maturin\", \"develop\", \"--release\"]\n\
             \n\
             [wheel]\n\
             group       = \"Build\"\n\
             description = \"Build distributable wheel\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\"]\n\
             command     = [\"maturin\", \"build\", \"--release\"]\n\
             \n\
             [check]\n\
             group       = \"Build\"\n\
             description = \"Fast type-check\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\"]\n\
             command     = [\"{{{{cargo}}}}\", \"check\"{ws_flag}]\n\
             \n\
             [test]\n\
             group       = \"Build\"\n\
             description = \"Run tests\"\n\
             type        = \"task\"\n\
             command     = [\"{{{{cargo}}}}\", \"test\"{ws_flag}]\n\
             \n\
             [clippy]\n\
             group       = \"Quality\"\n\
             description = \"Run clippy linter\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*.rs\", \"Cargo.toml\"]\n\
             command     = [\"{{{{cargo}}}}\", \"clippy\"{ws_flag}, \"--\", \"-D\", \"warnings\"]\n\
             \n\
             [fmt]\n\
             group       = \"Quality\"\n\
             description = \"Format Rust code\"\n\
             type        = \"task\"\n\
             command     = [\"{{{{cargo}}}}\", \"fmt\"]\n\
             \n\
             [clean]\n\
             description = \"Remove build artifacts\"\n\
             type        = \"task\"\n\
             command     = [\"{{{{cargo}}}}\", \"clean\"]\n\
             "
        ));
    }

    // ── Node / npm ────────────────────────────────────────────────────────────
    if has_npm && !has_cargo {
        // Detect yarn/pnpm.
        let pm = if std::path::Path::new("yarn.lock").exists() {
            "yarn"
        } else if std::path::Path::new("pnpm-lock.yaml").exists() {
            "pnpm"
        } else {
            "npm"
        };
        let run = if pm == "npm" { "npm run" } else { pm };
        out.push_str(&format!(
            "[build]\n\
             group       = \"Build\"\n\
             description = \"Build the project\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*\", \"package.json\"]\n\
             shell       = true\n\
             command     = [\"{run} build\"]\n\
             \n\
             [test]\n\
             group       = \"Build\"\n\
             description = \"Run tests\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*\", \"package.json\"]\n\
             shell       = true\n\
             command     = [\"{run} test\"]\n\
             \n\
             [lint]\n\
             group       = \"Quality\"\n\
             description = \"Run linter\"\n\
             type        = \"task\"\n\
             inputs      = [\"src/**/*\"]\n\
             shell       = true\n\
             command     = [\"{run} lint\"]\n\
             \n\
             [install]\n\
             group       = \"Setup\"\n\
             description = \"Install dependencies\"\n\
             type        = \"task\"\n\
             inputs      = [\"package.json\"]\n\
             command     = [\"{pm}\", \"install\"]\n\
             \n\
             [clean]\n\
             description = \"Remove build artifacts\"\n\
             type        = \"task\"\n\
             shell       = true\n\
             command     = [\"rm -rf dist build node_modules/.cache\"]\n\
             "
        ));
    }

    // ── Pure Python ───────────────────────────────────────────────────────────
    if has_python && !has_cargo {
        // Detect test runner.
        let uses_pytest = std::path::Path::new("pytest.ini").exists()
            || fs::read_to_string("pyproject.toml")
                .map(|s| s.contains("[tool.pytest"))
                .unwrap_or(false);
        let test_cmd = if uses_pytest {
            format!("{venv_python} -m pytest")
        } else {
            format!("{venv_python} -m unittest discover")
        };

        out.push_str(&format!(
            "[vars]\n\
             python = \"{venv_python}\"\n\
             \n\
             [test]\n\
             group       = \"Build\"\n\
             description = \"Run tests\"\n\
             type        = \"task\"\n\
             inputs      = [\"**/*.py\"]\n\
             shell       = true\n\
             command     = [\"{test_cmd}\"]\n\
             \n\
             [lint]\n\
             group       = \"Quality\"\n\
             description = \"Run ruff linter\"\n\
             type        = \"task\"\n\
             inputs      = [\"**/*.py\"]\n\
             shell       = true\n\
             command     = [\"{{{{python}}}} -m ruff check .\"]\n\
             \n\
             [fmt]\n\
             group       = \"Quality\"\n\
             description = \"Format Python code\"\n\
             type        = \"task\"\n\
             command     = [\"{{{{python}}}}\", \"-m\", \"ruff\", \"format\", \".\"]\n\
             \n\
             [clean]\n\
             description = \"Remove build artifacts\"\n\
             type        = \"task\"\n\
             shell       = true\n\
             command     = [\"rm -rf dist build *.egg-info __pycache__\"]\n\
             "
        ));
    }

    // ── Go ────────────────────────────────────────────────────────────────────
    if has_go {
        out.push_str(
            "[build]\n\
             group       = \"Build\"\n\
             description = \"Build the project\"\n\
             type        = \"task\"\n\
             inputs      = [\"**/*.go\", \"go.mod\", \"go.sum\"]\n\
             command     = [\"go\", \"build\", \"./...\"]\n\
             \n\
             [test]\n\
             group       = \"Build\"\n\
             description = \"Run tests\"\n\
             type        = \"task\"\n\
             inputs      = [\"**/*.go\"]\n\
             command     = [\"go\", \"test\", \"./...\"]\n\
             \n\
             [lint]\n\
             group       = \"Quality\"\n\
             description = \"Run golangci-lint\"\n\
             type        = \"task\"\n\
             inputs      = [\"**/*.go\"]\n\
             command     = [\"golangci-lint\", \"run\"]\n\
             \n\
             [fmt]\n\
             group       = \"Quality\"\n\
             description = \"Format Go code\"\n\
             type        = \"task\"\n\
             command     = [\"gofmt\", \"-w\", \".\"]\n\
             \n\
             [clean]\n\
             description = \"Remove build artifacts\"\n\
             type        = \"task\"\n\
             command     = [\"go\", \"clean\"]\n\
             ",
        );
    }

    // ── CMake ─────────────────────────────────────────────────────────────────
    if has_cmake {
        out.push_str(
            "[configure]\n\
             group       = \"Build\"\n\
             description = \"Configure with CMake\"\n\
             type        = \"task\"\n\
             inputs      = [\"CMakeLists.txt\"]\n\
             shell       = true\n\
             command     = [\"cmake -B build -DCMAKE_BUILD_TYPE=Release\"]\n\
             \n\
             [build]\n\
             group       = \"Build\"\n\
             description = \"Compile\"\n\
             type        = \"task\"\n\
             deps        = [\"configure\"]\n\
             shell       = true\n\
             command     = [\"cmake --build build --parallel\"]\n\
             \n\
             [test]\n\
             group       = \"Build\"\n\
             description = \"Run CTest\"\n\
             type        = \"task\"\n\
             deps        = [\"build\"]\n\
             shell       = true\n\
             command     = [\"ctest --test-dir build --output-on-failure\"]\n\
             \n\
             [clean]\n\
             description = \"Remove build directory\"\n\
             type        = \"task\"\n\
             shell       = true\n\
             command     = [\"rm -rf build\"]\n\
             ",
        );
    }

    // ── Plain Makefile fallback ───────────────────────────────────────────────
    if has_make && !has_cargo && !has_npm && !has_python && !has_go && !has_cmake {
        out.push_str(
            "[build]\n\
             group       = \"Build\"\n\
             description = \"Build (delegates to make)\"\n\
             type        = \"task\"\n\
             command     = [\"make\"]\n\
             \n\
             [test]\n\
             group       = \"Build\"\n\
             description = \"Run tests (delegates to make test)\"\n\
             type        = \"task\"\n\
             command     = [\"make\", \"test\"]\n\
             \n\
             [clean]\n\
             description = \"Clean (delegates to make clean)\"\n\
             type        = \"task\"\n\
             command     = [\"make\", \"clean\"]\n\
             ",
        );
    }

    // ── Blank fallback ────────────────────────────────────────────────────────
    if !has_cargo && !has_npm && !has_python && !has_go && !has_cmake && !has_make {
        out.push_str(INIT_TEMPLATE_RULES);
    }

    out
}

const INIT_TEMPLATE: &str = r#"# pbuild.toml
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

const INIT_TEMPLATE_RULES: &str = r#"[build]
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

fn cmd_status(target: Option<&str>, profile: Option<&str>) -> Result<()> {
    let mut bf = load_build_file()?;
    if let Some(p) = profile {
        apply_profile(&mut bf, p)?;
    }
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

fn cmd_prune() -> Result<()> {
    let mut lf = hash::read_lock_file()?;
    if lf.is_empty() {
        println!("lock file is empty — nothing to prune");
        return Ok(());
    }

    // Build the set of all paths still referenced by current rules.
    let referenced: std::collections::HashSet<String> = if let Ok(bf) = load_build_file() {
        bf.rules
            .values()
            .flat_map(|r| {
                r.inputs
                    .iter()
                    .cloned()
                    .chain(std::iter::once(r.output.clone()).filter(|s| !s.is_empty()))
                    .chain(
                        r.depfile
                            .iter()
                            .flat_map(|df| hash::load_depfile_inputs(&lf, df)),
                    )
            })
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    // Remove entries whose file is gone and isn't referenced by any rule.
    let stale: Vec<String> = lf
        .keys()
        .filter(|k| {
            // Always keep env: and dep: synthetic keys.
            if k.starts_with("env:") || k.starts_with("dep:") {
                return false;
            }
            let still_exists = std::path::Path::new(k.as_str()).exists();
            let still_referenced = referenced.contains(k.as_str());
            !still_exists && !still_referenced
        })
        .cloned()
        .collect();

    if stale.is_empty() {
        println!("nothing to prune");
        return Ok(());
    }

    for key in &stale {
        lf.remove(key);
        println!("pruned {key}");
    }
    hash::write_lock_file(&lf).context("failed to write lock file")?;
    println!("pruned {} stale entries", stale.len());
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

/// Remove a single target's output file and its lock file entries.
fn cmd_clean_target(target_name: &str) -> Result<()> {
    let bf = load_build_file()?;
    let raw = bf
        .rules
        .get(target_name)
        .ok_or_else(|| anyhow::anyhow!("no rule for target: {target_name}"))?;

    if !raw.output.is_empty() {
        remove_if_exists(&raw.output)?;
    }

    let mut lf = hash::read_lock_file()?;
    let inputs = expand_inputs(&raw.inputs)?;
    hash::remove_rule_entries(&mut lf, &inputs, &raw.output);
    hash::write_lock_file(&lf).context("failed to write lock file")?;
    println!("cleaned {target_name}");
    Ok(())
}

/// Hash a target's inputs and output right now and store in the lock file,
/// marking it clean without running its command.
fn cmd_touch(target_name: &str) -> Result<()> {
    let bf = load_build_file()?;
    let raw = bf
        .rules
        .get(target_name)
        .ok_or_else(|| anyhow::anyhow!("no rule for target: {target_name}"))?;

    let inputs = expand_inputs(&raw.inputs)?;
    let mut lf = hash::read_lock_file()?;

    let paths: Vec<&str> = inputs
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(raw.output.as_str()).filter(|s| !s.is_empty()))
        .collect();

    let mut touched = 0usize;
    for path in paths {
        match hash::hash_file(path)? {
            Some(h) => {
                lf.insert(path.to_string(), h);
                touched += 1;
            }
            None => eprintln!("pbuild touch: {path}: file not found, skipping"),
        }
    }

    hash::write_lock_file(&lf).context("failed to write lock file")?;
    println!("touched {target_name} ({touched} file(s) hashed)");
    Ok(())
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
                    _ => "   ",
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

/// Open `pbuild.toml` in `$EDITOR` positioned at the given target's rule.
fn cmd_edit(target: Option<&str>) -> Result<()> {
    if !std::path::Path::new("pbuild.toml").exists() {
        anyhow::bail!("no pbuild.toml found — run `pbuild init` first");
    }

    let src = fs::read_to_string("pbuild.toml").context("could not read pbuild.toml")?;

    // Find the line number of the target's rule header.
    let line_num: Option<usize> = target.and_then(|name| {
        let plain = format!("[{name}]");
        let quoted = format!("[\"{name}\"]");
        src.lines()
            .enumerate()
            .find(|(_, line)| {
                let t = line.trim();
                t == plain || t == quoted
            })
            .map(|(i, _)| i + 1)
    });

    // If target not found, offer to add it.
    if let (Some(name), None) = (target, line_num) {
        eprintln!("pbuild: target `{name}` not found in pbuild.toml");
        eprintln!("Run `pbuild add {name}` to create it.");
        return Ok(());
    }

    // Resolve editor: $EDITOR → $VISUAL → vi.
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    // Editor binary name (no path).
    let editor_bin = std::path::Path::new(&editor)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&editor)
        .to_string();

    let mut cmd = std::process::Command::new(&editor);

    if let Some(n) = line_num {
        // Pass +N line jump flag for editors that support it.
        if matches!(editor_bin.as_str(), "code" | "code-insiders") {
            // VS Code uses file:line syntax.
            cmd.arg(format!("pbuild.toml:{n}"));
        } else {
            // vim, nvim, nano, hx, emacs, and most others accept +N.
            cmd.arg(format!("+{n}"));
        }
    }

    // For VS Code we already passed the file in the line-jump arg above.
    if !matches!(editor_bin.as_str(), "code" | "code-insiders") {
        cmd.arg("pbuild.toml");
    }

    let status = cmd
        .status()
        .with_context(|| format!("failed to launch editor: {editor}"))?;

    if !status.success() {
        anyhow::bail!("editor exited with status {status}");
    }

    Ok(())
}

/// Parse a Makefile and emit an equivalent pbuild.toml to stdout (or write it
/// to pbuild.toml if that file doesn't exist yet).
///
/// Handles:
///   - Variable assignments (`VAR = value`, `VAR := value`, `VAR ?= value`)
///   - `.PHONY` declarations
///   - Target rules with dependencies and shell recipe lines
///
/// Skips / warns about:
///   - Pattern rules (`%.o: %.c`)
///   - Built-in / internal targets (`.SUFFIXES`, `.DEFAULT`, etc.)
///   - `$(shell ...)` expansions in variable values
///   - `ifeq`/`ifdef` conditional blocks
#[allow(clippy::too_many_lines)]
fn cmd_import(makefile_path: &str) -> Result<()> {
    let src = fs::read_to_string(makefile_path)
        .with_context(|| format!("could not read {makefile_path}"))?;

    let mut vars: Vec<(String, String)> = Vec::new();
    let mut phony: std::collections::HashSet<String> = std::collections::HashSet::new();
    // target -> (deps, commands)
    let mut targets: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // State machine: we're either between rules or collecting a recipe.
    let mut current_target: Option<(String, Vec<String>)> = None;
    let mut current_recipe: Vec<String> = Vec::new();

    let flush = |targets: &mut Vec<(String, Vec<String>, Vec<String>)>,
                 current_target: &mut Option<(String, Vec<String>)>,
                 current_recipe: &mut Vec<String>| {
        if let Some((name, deps)) = current_target.take() {
            targets.push((name, deps, std::mem::take(current_recipe)));
        }
    };

    for (lineno, raw_line) in src.lines().enumerate() {
        let lineno = lineno + 1;

        // Continuation lines — join them simply (we don't need to handle
        // multi-line recipes perfectly; just collect them).
        let line = raw_line.trim_end_matches('\\').trim_end();

        // Skip blank lines and comments.
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        // Recipe lines (indented with a real tab in the original).
        if raw_line.starts_with('\t') {
            if let Some((ref _name, _)) = current_target {
                // Strip leading @ (silencing prefix) and leading whitespace.
                let cmd = line.trim_start_matches('@').trim();
                // Skip $(MAKE) recursive calls — they need manual attention.
                if cmd.contains("$(MAKE)") || cmd.contains("${MAKE}") {
                    warnings.push(format!(
                        "line {lineno}: skipped recursive $(MAKE) call — convert manually"
                    ));
                    continue;
                }
                // Skip echo-only lines that are just decorative.
                if !cmd.is_empty() {
                    current_recipe.push(make_vars_to_pbuild(cmd));
                }
            }
            continue;
        }

        // .PHONY declaration.
        if line.trim_start().starts_with(".PHONY") {
            if let Some(rest) = line.find(':').map(|i| &line[i + 1..]) {
                for name in rest.split_whitespace() {
                    phony.insert(name.to_string());
                }
            }
            flush(&mut targets, &mut current_target, &mut current_recipe);
            continue;
        }

        // Skip built-in / internal targets.
        if line.trim_start().starts_with('.') {
            flush(&mut targets, &mut current_target, &mut current_recipe);
            continue;
        }

        // Skip ifeq/ifdef/else/endif.
        let trimmed = line.trim_start();
        if trimmed.starts_with("ifeq")
            || trimmed.starts_with("ifneq")
            || trimmed.starts_with("ifdef")
            || trimmed.starts_with("ifndef")
            || trimmed.starts_with("else")
            || trimmed.starts_with("endif")
        {
            warnings.push(format!(
                "line {lineno}: conditional `{trimmed}` — conditional logic cannot be converted; review manually"
            ));
            flush(&mut targets, &mut current_target, &mut current_recipe);
            continue;
        }

        // Variable assignment: VAR = val, VAR := val, VAR ?= val, VAR += val.
        if let Some(eq_pos) = find_assignment(line) {
            flush(&mut targets, &mut current_target, &mut current_recipe);
            let (lhs, op, rhs) = eq_pos;
            let val = rhs.trim();
            if val.contains("$(shell") || val.contains("${shell") {
                warnings.push(format!(
                    "line {lineno}: variable `{lhs}` uses $(shell ...) — set it manually or use a [vars] entry"
                ));
            } else if op != "+=" {
                // Only store simple assignments; += is too stateful to translate.
                // Expand any $(VAR) references in the value using already-known vars.
                let expanded = make_vars_to_pbuild(val);
                vars.push((lhs.trim().to_string(), expanded));
            }
            continue;
        }

        // Target rule: target: deps...
        if let Some(colon) = line.find(':') {
            // Skip pattern rules.
            let target_part = line[..colon].trim();
            if target_part.contains('%') {
                warnings.push(format!(
                    "line {lineno}: skipped pattern rule `{target_part}` — convert manually"
                ));
                flush(&mut targets, &mut current_target, &mut current_recipe);
                continue;
            }
            // Skip targets with variable expansions in name — too dynamic.
            if target_part.contains('$') {
                warnings.push(format!(
                    "line {lineno}: skipped dynamic target `{target_part}` — convert manually"
                ));
                flush(&mut targets, &mut current_target, &mut current_recipe);
                continue;
            }
            flush(&mut targets, &mut current_target, &mut current_recipe);
            let deps_part = line[colon + 1..].trim();
            // Skip double-colon rules (rare).
            let deps_part = deps_part.trim_start_matches(':');
            let deps: Vec<String> = deps_part
                .split_whitespace()
                .filter(|d| !d.starts_with('$')) // skip make-variable deps
                .map(ToString::to_string)
                .collect();
            current_target = Some((target_part.to_string(), deps));
            current_recipe = Vec::new();
        }
    }
    // Flush the last target.
    {
        if let Some((name, deps)) = current_target.take() {
            targets.push((name, deps, current_recipe));
        }
    }

    // ── Emit pbuild.toml ──────────────────────────────────────────────────────
    let mut out = String::from(
        "# pbuild.toml — generated by `pbuild import`\n# Review and adjust before use.\n\n",
    );

    // Determine default: the first non-phony, non-special target, or first phony.
    let default_target = targets
        .iter()
        .find(|(name, _, _)| name != "all" && name != "help")
        .or_else(|| targets.first())
        .map_or("build", |(name, _, _)| name.as_str());

    let _ = writeln!(out, "[config]\ndefault = \"{default_target}\"\n");

    // Emit [vars] for simple variable assignments.
    let emittable_vars: Vec<&(String, String)> = vars
        .iter()
        .filter(|(_, v)| !v.contains('$') && !v.is_empty())
        .collect();
    if !emittable_vars.is_empty() {
        out.push_str("[vars]\n");
        for (k, v) in &emittable_vars {
            let _ = writeln!(out, "{k} = \"{v}\"");
        }
        out.push('\n');
    }

    // Set of all known target names (for dep filtering).
    let known_targets: std::collections::HashSet<&str> =
        targets.iter().map(|(n, _, _)| n.as_str()).collect();

    for (name, deps, recipe) in &targets {
        // Determine type: phony if in .PHONY or has no output-like name.
        let is_phony = phony.contains(name.as_str())
            || !name.contains('.')   // names without extension are usually phony
            || recipe.is_empty();

        // Sanitize the TOML key.
        let key = if name.contains('.') || name.contains('/') || name.contains(' ') {
            format!("[\"{name}\"]")
        } else {
            format!("[{name}]")
        };
        let _ = writeln!(out, "{key}");
        if is_phony {
            out.push_str("type    = \"task\"\n");
        }

        // Filter deps to only known targets (skip source files like %.c).
        let rule_deps: Vec<&str> = deps
            .iter()
            .filter(|d| known_targets.contains(d.as_str()))
            .map(String::as_str)
            .collect();
        if !rule_deps.is_empty() {
            let dep_list = rule_deps
                .iter()
                .map(|d| format!("\"{d}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "deps    = [{dep_list}]");
        }

        if recipe.is_empty() && !rule_deps.is_empty() {
            // Aggregate target: deps but no recipe — runs deps then exits cleanly.
            out.push_str("command = [\"true\"]\n");
        } else if recipe.is_empty() {
            // Stub target: no deps, no recipe — needs manual filling.
            out.push_str("# no commands — add a command field\n");
            out.push_str("command = [\"echo\", \"TODO\"]\n");
        } else if recipe.len() == 1 {
            let argv = shell_split(&recipe[0]);
            let toml_argv = argv
                .iter()
                .map(|a| format!("\"{}\"", a.replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "command = [{toml_argv}]");
        } else {
            out.push_str("commands = [\n");
            for cmd in recipe {
                let argv = shell_split(cmd);
                let toml_argv = argv
                    .iter()
                    .map(|a| format!("\"{}\"", a.replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "    [{toml_argv}],");
            }
            out.push_str("]\n");
        }
        out.push('\n');
    }

    // ── Print warnings ────────────────────────────────────────────────────────
    for w in &warnings {
        eprintln!("pbuild import: warning: {w}");
    }

    // ── Write output ──────────────────────────────────────────────────────────
    if std::path::Path::new("pbuild.toml").exists() {
        // Print to stdout so the user can review before overwriting.
        print!("{out}");
        eprintln!("pbuild import: pbuild.toml already exists — printed to stdout instead.");
    } else {
        fs::write("pbuild.toml", &out).context("failed to write pbuild.toml")?;
        println!("wrote pbuild.toml");
        if !warnings.is_empty() {
            eprintln!(
                "pbuild import: {} warning(s) — review pbuild.toml before use",
                warnings.len()
            );
        }
    }

    Ok(())
}

/// Find a variable assignment in `line`. Returns `(lhs, op, rhs)` or None.
fn find_assignment(line: &str) -> Option<(String, &'static str, String)> {
    // Try each operator in order (longest first to avoid ambiguity).
    for op in &[":=", "?=", "+=", "!=", "="] {
        if let Some(pos) = line.find(op) {
            let lhs = &line[..pos];
            // Make sure lhs has no colon (that would make it a target rule).
            if lhs.contains(':') {
                return None;
            }
            let rhs = &line[pos + op.len()..];
            return Some((lhs.to_string(), op, rhs.to_string()));
        }
    }
    None
}

/// Convert Make-style `$(VAR)` and `${VAR}` references to pbuild `{{VAR}}`.
/// Leaves `$(shell ...)` and complex expansions unchanged (they'll need manual review).
fn make_vars_to_pbuild(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for $( or ${
        if bytes[i] == b'$' && i + 1 < bytes.len() && (bytes[i + 1] == b'(' || bytes[i + 1] == b'{') {
            let close = if bytes[i + 1] == b'(' { b')' } else { b'}' };
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == close) {
                let var_name = &s[i + 2..i + 2 + end];
                // Skip function calls like $(shell ...), $(wildcard ...), $(patsubst ...)
                let is_func = var_name.contains(' ') || var_name.contains('\t');
                if is_func {
                    out.push_str(&s[i..i + 2 + end + 1]);
                } else {
                    out.push_str("{{");
                    out.push_str(var_name);
                    out.push_str("}}");
                }
                i += 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Very simple shell tokenizer: splits on whitespace, handling single and
/// double quotes. Good enough for Makefile recipe lines.
fn shell_split(cmd: &str) -> Vec<String> {
    // If the command contains shell metacharacters, return it as a single
    // token so the rule gets `shell = true` treatment (we note it's complex).
    // Actually we just return the whole thing to preserve intent.
    if cmd.contains("&&")
        || cmd.contains("||")
        || cmd.contains(">>")
        || cmd.contains(';')
        || cmd.contains('|')
    {
        return vec![cmd.to_string()];
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in cmd.chars() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Extract `--profile <name>` or `-p <name>` from a raw argv slice.
fn extract_profile(argv: &[String]) -> Option<&str> {
    let mut iter = argv.iter();
    while let Some(a) = iter.next() {
        if a == "--profile" || a == "-p" {
            return iter.next().map(String::as_str);
        }
        if let Some(val) = a.strip_prefix("--profile=") {
            return Some(val);
        }
    }
    None
}

#[allow(clippy::too_many_lines)]
fn run() -> Result<()> {
    // Detect `why` and `status` before full arg parsing — they take their own positional argument.
    let raw_argv: Vec<String> = std::env::args().skip(1).collect();
    let early_profile = extract_profile(&raw_argv);

    if raw_argv.first().map(String::as_str) == Some("why") {
        let target = raw_argv
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-') && *a != early_profile.unwrap_or(""))
            .map(String::as_str)
            .ok_or_else(|| anyhow::anyhow!("usage: pbuild why <target>"))?;
        return cmd_why(target, early_profile);
    }
    if raw_argv.first().map(String::as_str) == Some("status") {
        let target = raw_argv
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-') && *a != early_profile.unwrap_or(""))
            .map(String::as_str);
        return cmd_status(target, early_profile);
    }
    if raw_argv.first().map(String::as_str) == Some("add") {
        let name = raw_argv
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("usage: pbuild add <name>"))?;
        return cmd_add(name);
    }
    if raw_argv.first().map(String::as_str) == Some("import") {
        let path = raw_argv.get(1).map_or("Makefile", String::as_str);
        return cmd_import(path);
    }
    if raw_argv.first().map(String::as_str) == Some("edit") {
        let target = raw_argv.get(1).map(String::as_str);
        return cmd_edit(target);
    }
    // `pbuild clean <target>` — clean just one target.
    // `pbuild clean` (no arg) — handled later via args.target == "clean".
    if raw_argv.first().map(String::as_str) == Some("clean") {
        if let Some(target) = raw_argv.get(1) {
            return cmd_clean_target(target);
        }
        // Fall through to the normal clean path.
    }
    if raw_argv.first().map(String::as_str) == Some("prune") {
        return cmd_prune();
    }
    if raw_argv.first().map(String::as_str) == Some("touch") {
        let target = raw_argv
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("usage: pbuild touch <target>"))?;
        return cmd_touch(target);
    }
    if raw_argv.first().map(String::as_str) == Some("graph") {
        let dot = raw_argv.iter().any(|a| a == "--dot");
        let target = raw_argv
            .iter()
            .skip(1)
            .find(|a| *a != "--dot")
            .map(String::as_str);
        let bf = load_build_file()?;
        let rules = to_rules(&bf)?;
        let root = resolve_target(&bf, target)?;
        if dot {
            print_dot(&rules, &root);
        } else {
            print_graph(&rules, &root);
        }
        return Ok(());
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
        return cmd_init(args.detect);
    }

    let mut bf = load_build_file()?;

    if let Some(profile) = &args.profile {
        apply_profile(&mut bf, profile)?;
    }

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
        extra_args: args.extra_args.clone(),
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

    // Safety check only the rules that will actually run.
    if !args.trust && !bf.config.trust {
        let warnings = safety_warnings(&plan);
        if !warnings.is_empty() {
            for w in &warnings {
                eprintln!("pbuild: unsafe: {w}");
            }
            eprintln!("pbuild: refusing to run. review the commands and pass --trust to proceed.");
            return Err(anyhow::anyhow!("unsafe commands detected"));
        }
    }

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
