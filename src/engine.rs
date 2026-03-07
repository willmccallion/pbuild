use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;

use crate::depfile;
use crate::hash::{self, LockFile};
use crate::process::run_command;
use crate::types::{Rule, Target};
use crate::ui;

pub struct Config {
    /// Max concurrent rules.
    pub jobs: usize,
    /// Print commands without executing them.
    pub dry_run: bool,
    /// Print [skip] lines and extra info.
    pub verbose: bool,
    /// Keep building independent rules after a failure.
    pub keep_going: bool,
    /// Environment variables that trigger a full rebuild when changed.
    pub env: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            jobs: 4,
            dry_run: false,
            verbose: false,
            keep_going: false,
            env: Vec::new(),
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
        ui::print_env_dirty();
    }

    // The plan is already topologically sorted (leaves first).
    // We process it in waves: collect all rules whose deps are done,
    // run them in parallel, mark them done, repeat.
    let mut done: HashSet<Target> = HashSet::new();
    let mut remaining: Vec<&Rule> = rules.iter().collect();
    let mut failures: Vec<anyhow::Error> = Vec::new();

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
        let results: Vec<Result<()>> = pool.install(|| {
            ready
                .par_iter()
                .map(|rule| run_rule(cfg, env_dirty, &lock_file, &rebuilt, rule))
                .collect()
        });

        for (rule, res) in ready.iter().zip(results) {
            match res.with_context(|| format!("rule failed for target: {}", rule.target)) {
                Ok(()) => {
                    done.insert(rule.target.clone());
                }
                Err(e) if cfg.keep_going => {
                    ui::print_fail(&rule.target);
                    eprintln!("pbuild: {e}");
                    failures.push(e);
                }
                Err(e) => return Err(e),
            }
        }

        // Flush lock file once per wave rather than after every rule.
        hash::write_lock_file(&lock_file.read().unwrap()).context("failed to write lock file")?;

        remaining = not_ready;
    }

    if !failures.is_empty() {
        anyhow::bail!("{} rule(s) failed", failures.len());
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

fn run_rule(
    cfg: &Config,
    env_dirty: bool,
    lock_file: &RwLock<LockFile>,
    rebuilt: &Mutex<HashSet<Target>>,
    rule: &Rule,
) -> Result<()> {
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
            ui::print_skip(&rule.target);
        }
        return Ok(());
    }

    // Build the final command list, injecting -MF into the last command if
    // a depfile is declared (mirrors compiler convention: flags come last).
    let last_idx = rule.commands.len() - 1;
    let commands: Vec<Vec<String>> = rule
        .commands
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            if i == last_idx {
                if let Some(df) = &rule.depfile {
                    return cmd
                        .iter()
                        .cloned()
                        .chain(["-MF".to_string(), df.clone()])
                        .collect();
                }
            }
            cmd.clone()
        })
        .collect();

    if cfg.dry_run {
        for cmd in &commands {
            ui::print_dry_run(cmd);
        }
        return Ok(());
    }

    let start = Instant::now();
    for cmd in &commands {
        ui::print_command(cmd);
        run_command(cmd)?;
    }
    ui::print_done(&rule.target, start.elapsed());

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

    Ok(())
}

/// True if any of the given files are dirty relative to the lock file.
/// No declared inputs → always run (returns true).
fn any_dirty(lock_file: &RwLock<LockFile>, inputs: &[String]) -> Result<bool> {
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
