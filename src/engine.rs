use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;

use crate::depfile;
use crate::hash::{self, LockFile};
use crate::process::{run_command, run_command_streaming, run_command_tty};
use crate::types::{Rule, Target};
use crate::ui::UiConfig;

pub struct Config {
    /// Max concurrent rules.
    pub jobs: usize,
    /// Print commands without executing them.
    pub dry_run: bool,
    /// Print skip lines and extra info.
    pub verbose: bool,
    /// Keep building independent rules after a failure.
    pub keep_going: bool,
    /// Environment variables that trigger a full rebuild when changed.
    pub env: Vec<String>,
    /// Terminal output settings.
    pub ui: UiConfig,
    /// Extra arguments passed after `--` on the CLI. Appended to the last
    /// command of the target rule, or inserted at `{{args}}` if present.
    pub extra_args: Vec<String>,
    /// Suppress pbuild's own status lines (› start, $ cmd, ✓ done).
    /// Command output is still shown.
    pub quiet: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            jobs: 4,
            dry_run: false,
            verbose: false,
            keep_going: false,
            env: Vec::new(),
            ui: UiConfig {
                color: None,
                prefix: None,
                log: None,
            },
            extra_args: Vec::new(),
            quiet: false,
        }
    }
}

/// Execute a topologically sorted build plan with bounded parallelism.
///
/// A rule is skipped when ALL of the following hold:
///   1. All inputs are clean (hash matches the lock file).
///   2. No dep target was rebuilt in this invocation (cascade propagation).
///
/// After a successful action, inputs and output are hashed and persisted
/// to `.pbuild.lock` for use in future invocations.
pub fn execute_plan(cfg: &Config, rules: &[Rule]) -> Result<()> {
    if rules.is_empty() {
        return Ok(());
    }

    let pool = ThreadPoolBuilder::new()
        .num_threads(cfg.jobs)
        .build()
        .context("failed to build thread pool")?;

    // lock_file: read-heavy (dirty checks), write-rare (after each rule succeeds).
    // rebuilt: written after each rule, read when checking dep cascade.
    let lock_file: Arc<RwLock<LockFile>> = Arc::new(RwLock::new(hash::read_lock_file()?));
    let rebuilt: Arc<Mutex<HashSet<Target>>> = Arc::new(Mutex::new(HashSet::new()));

    // If any tracked env var changed, treat every rule as dirty this run.
    let env_dirty = {
        let lf = lock_file.read().unwrap();
        cfg.env.iter().any(|var| hash::env_is_dirty(&lf, var))
    };
    if env_dirty && cfg.verbose {
        cfg.ui.print_env_dirty();
    }

    // The plan is already topologically sorted (leaves first).
    // We process it in waves: collect all rules whose deps are done,
    // run them in parallel, mark them done, repeat.
    let mut done: HashSet<Target> = HashSet::new();
    let mut remaining: Vec<&Rule> = rules.iter().collect();
    let mut failures: Vec<anyhow::Error> = Vec::new();
    let mut timings: Vec<(String, std::time::Duration)> = Vec::new();

    while !remaining.is_empty() {
        // Collect the ready wave — skip rules whose deps failed.
        let (ready, not_ready): (Vec<_>, Vec<_>) = remaining
            .into_iter()
            .partition(|r| r.deps.iter().all(|d| done.contains(d)));

        if ready.is_empty() {
            if failures.is_empty() {
                anyhow::bail!("dependency deadlock — build plan may be invalid");
            }
            // Remaining rules are blocked by failed deps; stop here.
            break;
        }

        // Run the wave in parallel (bounded by the thread pool).
        // When only one rule is ready, stream its output live; when multiple
        // rules run simultaneously, buffer each atomically to prevent interleaving.
        let ui = &cfg.ui;
        let streaming = ready.len() == 1;
        let results: Vec<Result<Option<std::time::Duration>>> = pool.install(|| {
            ready
                .par_iter()
                .map(|rule| run_rule(cfg, env_dirty, streaming, ui, &lock_file, &rebuilt, rule))
                .collect()
        });

        for (rule, res) in ready.iter().zip(results) {
            match res.with_context(|| format!("rule failed for target: {}", rule.target)) {
                Ok(Some(elapsed)) => {
                    timings.push((rule.target.to_string(), elapsed));
                    done.insert(rule.target.clone());
                }
                Ok(None) => {
                    done.insert(rule.target.clone());
                }
                Err(e) if cfg.keep_going => {
                    if !cfg.quiet {
                        cfg.ui.print_fail(&rule.target);
                    }
                    eprintln!("pbuild: {e}");
                    // Record the failed target for `pbuild retry`.
                    let _ = std::fs::write(".pbuild.last-failed", rule.target.to_string());
                    failures.push(e);
                }
                Err(e) => {
                    // Record the failed target for `pbuild retry`.
                    let _ = std::fs::write(".pbuild.last-failed", rules.last().map(|r| r.target.to_string()).unwrap_or_default());
                    return Err(e);
                }
            }
        }

        // Flush lock file once per wave rather than after every rule.
        hash::write_lock_file(&lock_file.read().unwrap()).context("failed to write lock file")?;

        remaining = not_ready;
    }

    if !failures.is_empty() {
        anyhow::bail!("{} rule(s) failed", failures.len());
    }

    // Print timing summary if more than one rule ran.
    if timings.len() > 1 {
        timings.sort_by(|a, b| b.1.cmp(&a.1));
        cfg.ui.print_timing_summary(&timings);
    }

    // Persist env values so a future run can detect changes.
    if !cfg.env.is_empty() {
        let mut lf = lock_file.write().unwrap();
        for var in &cfg.env {
            if let Ok(val) = std::env::var(var) {
                lf.insert(hash::env_key(var), val);
            }
        }
        hash::write_lock_file(&lf).context("failed to write lock file")?;
    }

    Ok(())
}

/// Returns the elapsed time if the rule ran, or `None` if it was skipped.
#[allow(clippy::too_many_lines)]
fn run_rule(
    cfg: &Config,
    env_dirty: bool,
    streaming: bool,
    ui: &UiConfig,
    lock_file: &RwLock<LockFile>,
    rebuilt: &Mutex<HashSet<Target>>,
    rule: &Rule,
) -> Result<Option<std::time::Duration>> {
    // Merge declared inputs with any previously discovered depfile inputs.
    let all_inputs: Vec<String> = {
        let lf = lock_file.read().unwrap();
        let dep_inputs = rule
            .depfile
            .as_deref()
            .map(|_| hash::load_depfile_inputs(&lf, &rule.output))
            .unwrap_or_default();
        rule.inputs.iter().cloned().chain(dep_inputs).collect()
    };

    let file_dirty = any_dirty(lock_file, &all_inputs)?;
    let dep_rebuilt = {
        let r = rebuilt.lock().unwrap();
        rule.deps.iter().any(|d| r.contains(d))
    };

    if !file_dirty && !dep_rebuilt && !env_dirty {
        if cfg.verbose {
            ui.print_skip(&rule.target);
        }
        return Ok(None);
    }

    // In verbose mode, explain why this rule is dirty before running it.
    if cfg.verbose {
        if env_dirty {
            ui.print_dirty_reason(&rule.target, "env vars changed");
        } else if dep_rebuilt {
            let dep = rule
                .deps
                .iter()
                .find(|d| rebuilt.lock().unwrap().contains(*d))
                .map(|d| d.to_string())
                .unwrap_or_default();
            ui.print_dirty_reason(&rule.target, &format!("dep rebuilt: {dep}"));
        } else {
            // file_dirty: find and report the first changed input.
            let lf = lock_file.read().unwrap();
            let reason = all_inputs
                .iter()
                .find(|p| hash::is_dirty(&lf, p).unwrap_or(true))
                .map(|p| format!("changed: {p}"))
                .unwrap_or_else(|| "no inputs — always runs".to_string());
            drop(lf);
            ui.print_dirty_reason(&rule.target, &reason);
        }
    }

    // Build the final command list, injecting -MF and extra_args into the
    // last command if declared (mirrors compiler convention: flags come last).
    let last_idx = rule.commands.len() - 1;
    let commands: Vec<Vec<String>> = rule
        .commands
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            if i == last_idx {
                let mut built: Vec<String> = cmd
                    .iter()
                    .flat_map(|tok| {
                        if tok == "{{args}}" {
                            cfg.extra_args.clone()
                        } else {
                            vec![tok.clone()]
                        }
                    })
                    .collect();
                // If no {{args}} placeholder was expanded, append extra_args at the end.
                if !cfg.extra_args.is_empty() && !cmd.iter().any(|t| t == "{{args}}") {
                    built.extend(cfg.extra_args.iter().cloned());
                }
                if let Some(df) = &rule.depfile {
                    built.extend(["-MF".to_string(), df.clone()]);
                }
                return built;
            }
            cmd.clone()
        })
        .collect();

    if cfg.dry_run {
        ui.print_start(&rule.target);
        for cmd in &commands {
            if rule.shell {
                ui.print_dry_run(&[cmd.join(" ")]);
            } else {
                ui.print_dry_run(cmd);
            }
        }
        return Ok(None);
    }

    if !cfg.quiet {
        ui.print_start(&rule.target);
    }
    let start = Instant::now();
    let mut captured: Vec<u8> = Vec::new();

    // If this rule delegates to a subdirectory, override the command list.
    let subdir_commands: Vec<Vec<String>>;
    let effective_commands: &[Vec<String>];
    let effective_dir: Option<&str>;

    if let Some(dir) = rule.subdir.as_deref() {
        // Prefer pbuild if a pbuild.toml exists, else fall back to make.
        let has_pbuild = std::path::Path::new(dir).join("pbuild.toml").exists();
        let tool = if has_pbuild { "pbuild" } else { "make" };
        let target_str = rule.target.to_string();
        let cmd = vec![tool.to_string(), target_str];
        subdir_commands = vec![cmd];
        effective_commands = &subdir_commands;
        effective_dir = Some(dir);
    } else if let Some(dir) = rule.makedir.as_deref() {
        let target_str = rule.target.to_string();
        let cmd = vec!["make".to_string(), target_str];
        subdir_commands = vec![cmd];
        effective_commands = &subdir_commands;
        effective_dir = Some(dir);
    } else {
        effective_commands = &commands;
        effective_dir = rule.dir.as_deref();
    }

    let mut run_err: Option<anyhow::Error> = None;
    for cmd in effective_commands {
        let effective: Vec<String> =
            if rule.shell && rule.subdir.is_none() && rule.makedir.is_none() {
                vec!["sh".to_string(), "-c".to_string(), cmd.join(" ")]
            } else {
                cmd.clone()
            };
        if !cfg.quiet {
            ui.print_command(&effective);
        }
        if rule.tty {
            // tty = true: connect stdin/stdout/stderr directly to the terminal.
            if let Err(e) = run_command_tty(&effective, effective_dir, &rule.env) {
                run_err = Some(e);
                break;
            }
        } else if streaming && !cfg.dry_run {
            // Single-rule wave: stream output directly to the terminal so the
            // user sees progress in real time instead of waiting for completion.
            if let Err(e) = run_command_streaming(&effective, effective_dir, &rule.env) {
                run_err = Some(e);
                break;
            }
        } else {
            match run_command(&effective, effective_dir, &rule.env) {
                Ok(output) => captured.extend_from_slice(&output),
                Err(e) => {
                    // Extract any output embedded in the error message and print it
                    // before surfacing the failure, so the user sees compiler errors.
                    let msg = e.to_string();
                    // run_command embeds output after the first newline.
                    if let Some(pos) = msg.find('\n') {
                        let output_part = &msg[pos + 1..];
                        if !output_part.is_empty() {
                            captured.extend_from_slice(output_part.as_bytes());
                        }
                    }
                    run_err = Some(e);
                    break;
                }
            }
        }
    }
    // Print buffered output (only non-empty when not streaming).
    if !captured.is_empty() {
        // In quiet mode print output without indentation prefix.
        if cfg.quiet {
            let _ = std::io::Write::write_all(&mut std::io::stdout(), &captured);
        } else {
            ui.print_output(&captured);
        }
    }
    if let Some(e) = run_err {
        if !cfg.quiet {
            ui.print_fail(&rule.target);
        }
        return Err(e);
    }
    if !cfg.quiet {
        ui.print_done(&rule.target, start.elapsed());
    }

    // Parse depfile (if any) and discover additional inputs.
    let discovered: Vec<String> = match &rule.depfile {
        Some(df_path) => match std::fs::read_to_string(df_path) {
            Ok(src) => depfile::parse(&src),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("failed to read depfile {df_path}"));
            }
        },
        None => Vec::new(),
    };

    // Hash declared inputs, depfile-discovered inputs, and the output.
    let paths_to_hash: Vec<String> = rule
        .inputs
        .iter()
        .cloned()
        .chain(discovered.iter().cloned())
        .chain(std::iter::once(rule.output.clone()))
        .filter(|s| !s.is_empty())
        .collect();

    {
        let mut lf = lock_file.write().unwrap();
        for path in &paths_to_hash {
            if let Some(h) = hash::hash_file(path)? {
                lf.insert(path.clone(), h);
            }
        }
        // Persist the discovered paths so next run can merge them in.
        if !discovered.is_empty() {
            hash::store_depfile_inputs(&mut lf, &rule.output, &discovered);
        }
    }

    rebuilt.lock().unwrap().insert(rule.target.clone());

    Ok(Some(start.elapsed()))
}

/// Check dirty state for a plan without running anything.
/// Returns `(target_name, would_rebuild)` for each rule in plan order.
pub fn check_status(rules: &[Rule]) -> Result<Vec<(String, bool)>> {
    let lf = hash::read_lock_file()?;
    let lock_file = RwLock::new(lf);
    let mut results = Vec::new();

    for rule in rules {
        let all_inputs: Vec<String> = {
            let lf = lock_file.read().unwrap();
            let dep_inputs = rule
                .depfile
                .as_deref()
                .map(|_| hash::load_depfile_inputs(&lf, &rule.output))
                .unwrap_or_default();
            rule.inputs.iter().cloned().chain(dep_inputs).collect()
        };
        let dirty = any_dirty_lf(&lock_file, &all_inputs)?;
        results.push((rule.target.to_string(), dirty));
    }

    Ok(results)
}

/// True if any of the given files are dirty relative to the lock file.
/// No declared inputs → always run (returns true).
fn any_dirty(lock_file: &RwLock<LockFile>, inputs: &[String]) -> Result<bool> {
    any_dirty_lf(lock_file, inputs)
}

fn any_dirty_lf(lock_file: &RwLock<LockFile>, inputs: &[String]) -> Result<bool> {
    if inputs.is_empty() {
        return Ok(true);
    }
    let lf = lock_file.read().unwrap();
    for path in inputs {
        if hash::is_dirty(&lf, path)? {
            return Ok(true);
        }
    }
    Ok(false)
}
