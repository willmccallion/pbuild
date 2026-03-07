use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rayon::ThreadPoolBuilder;
use rayon::prelude::*;

use crate::hash::{self, LockFile};
use crate::process::run_command;
use crate::types::{Rule, Target};

pub struct Config {
    /// Max concurrent rules.
    pub jobs: usize,
    /// Print commands without executing them.
    pub dry_run: bool,
    /// Print [skip] lines and extra info.
    pub verbose: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            jobs: 4,
            dry_run: false,
            verbose: false,
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

    // Shared state — both protected by a single Mutex for simplicity.
    // The engine spends almost all its time in subprocesses, so lock
    // contention on these tiny maps is negligible.
    let lock_file: Arc<Mutex<LockFile>> = Arc::new(Mutex::new(hash::read_lock_file()?));
    let rebuilt: Arc<Mutex<HashSet<Target>>> = Arc::new(Mutex::new(HashSet::new()));

    // The plan is already topologically sorted (leaves first).
    // We process it in waves: collect all rules whose deps are done,
    // run them in parallel, mark them done, repeat.
    let mut done: HashSet<Target> = HashSet::new();
    let mut remaining: Vec<&Rule> = rules.iter().collect();

    while !remaining.is_empty() {
        // Collect the ready wave.
        let (ready, not_ready): (Vec<_>, Vec<_>) = remaining
            .into_iter()
            .partition(|r| r.deps.iter().all(|d| done.contains(d)));

        if ready.is_empty() {
            // Shouldn't happen if the plan is correctly sorted, but guard anyway.
            anyhow::bail!("dependency deadlock — build plan may be invalid");
        }

        // Run the wave in parallel (bounded by the thread pool).
        let results: Vec<Result<()>> = pool.install(|| {
            ready
                .par_iter()
                .map(|rule| run_rule(cfg, &lock_file, &rebuilt, rule))
                .collect()
        });

        // Propagate any errors.
        for (rule, res) in ready.iter().zip(results) {
            res.with_context(|| format!("rule failed for target: {}", rule.target))?;
            done.insert(rule.target.clone());
        }

        remaining = not_ready;
    }

    Ok(())
}

fn run_rule(
    cfg: &Config,
    lock_file: &Mutex<LockFile>,
    rebuilt: &Mutex<HashSet<Target>>,
    rule: &Rule,
) -> Result<()> {
    let file_dirty = any_dirty(lock_file, &rule.inputs)?;
    let dep_rebuilt = {
        let r = rebuilt.lock().unwrap();
        rule.deps.iter().any(|d| r.contains(d))
    };

    if !file_dirty && !dep_rebuilt {
        if cfg.verbose {
            println!("[skip] {}", rule.target);
        }
        return Ok(());
    }

    if cfg.dry_run {
        println!("{}", rule.command.join(" "));
        return Ok(());
    }

    println!("+ {}", rule.command.join(" "));
    run_command(&rule.command)?;

    // Hash inputs + output and flush lock file.
    let paths_to_hash: Vec<&str> = rule
        .inputs
        .iter()
        .map(|s| s.as_str())
        .chain(std::iter::once(rule.output.as_str()))
        .filter(|s| !s.is_empty())
        .collect();

    for path in paths_to_hash {
        if let Some(h) = hash::hash_file(path)? {
            let mut lf = lock_file.lock().unwrap();
            lf.insert(path.to_string(), h);
            hash::write_lock_file(&lf)?;
        }
    }

    rebuilt.lock().unwrap().insert(rule.target.clone());

    Ok(())
}

/// True if any of the given files are dirty relative to the lock file.
/// No declared inputs → always run (returns true).
fn any_dirty(lock_file: &Mutex<LockFile>, inputs: &[String]) -> Result<bool> {
    if inputs.is_empty() {
        return Ok(true);
    }
    let lf = lock_file.lock().unwrap();
    for path in inputs {
        if hash::is_dirty(&lf, path)? {
            return Ok(true);
        }
    }
    Ok(false)
}
