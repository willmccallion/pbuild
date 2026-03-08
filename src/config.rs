use std::collections::HashMap;
use std::fs;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::types::{Download, OutputMode, Rule, Target};

/// Expand a list of glob patterns into concrete file paths.
/// Exposed so callers outside `config` (e.g. `pbuild why`) can reuse it.
pub fn expand_inputs(patterns: &[String]) -> Result<Vec<String>> {
    expand_globs(patterns)
}

/// Interpolate `{{name}}` placeholders in a single string using the given vars table.
/// Falls back to environment variables for unknown keys.
/// Exposed for use by `pbuild doctor`.
pub fn interpolate_pub(vars: &std::collections::HashMap<String, String>, s: &str) -> String {
    interpolate(vars, s, true)
}

fn expand_globs(patterns: &[String]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for pattern in patterns {
        let matches: Vec<_> = glob::glob(pattern)
            .with_context(|| format!("invalid glob pattern: {pattern}"))?
            .collect::<Result<_, _>>()
            .with_context(|| format!("error reading glob pattern: {pattern}"))?;

        if matches.is_empty() {
            // Only warn for actual glob patterns (containing * or ?).
            // Literal paths (no wildcards) are kept as-is so the engine
            // can report a meaningful "missing input" error.
            if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
                eprintln!("pbuild: warning: glob '{pattern}' matched no files");
            }
            paths.push(pattern.clone());
        } else {
            for path in matches {
                paths.push(
                    path.to_str()
                        .with_context(|| format!("non-UTF-8 path matched by {pattern}"))?
                        .to_string(),
                );
            }
        }
    }
    Ok(paths)
}

/// Parsed `pbuild.toml`.
///
/// ```toml
/// [config]
/// default = "app"
///
/// [vars]
/// cargo = "cargo"
///
/// ["main.o"]
/// command = ["{{cargo}}", "build"]
/// inputs  = ["main.c"]
/// output  = "main.o"
/// ```
///
/// Rules default to `type = "file"`. Set `type = "task"` for phony targets
/// that should always run and are never hashed.
pub struct BuildFile {
    pub config: BuildConfig,
    pub ui: crate::ui::UiConfig,
    /// Variable definitions from `[vars]`. Used for `{{name}}` interpolation.
    pub vars: HashMap<String, String>,
    pub rules: HashMap<String, RawRule>,
}

#[derive(Debug, Default, Deserialize)]
pub struct BuildConfig {
    /// Target to build when none is specified on the CLI.
    pub default: Option<String>,
    /// Default number of parallel jobs (overridden by `-j` on the CLI).
    pub jobs: Option<usize>,
    /// Environment variables that trigger a full rebuild when their value changes.
    #[serde(default)]
    pub env: Vec<String>,
    /// Skip safety checks for dangerous commands (sudo, rm -rf, etc.).
    /// Equivalent to passing `--trust` on the CLI.
    #[serde(default)]
    pub trust: bool,
    /// Default timeout for all rules. Overridden per-rule by `max_time`.
    /// Accepts "5m", "30s", "1h", or a plain integer (seconds).
    pub max_time: Option<String>,
    /// Named profiles that can be activated with `--profile <name>`.
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

/// A named profile — activated with `--profile <name>`.
///
/// Profile values are merged on top of `[config]` and `[vars]`:
/// - `jobs`, `default`, `trust`: replace the base value when set.
/// - `env`: appended to the base list.
/// - `vars`: merged into `[vars]`, with profile values taking precedence.
///
/// ```toml
/// [config.profiles.ci]
/// jobs = 1
/// vars = { cargo = "cargo +stable" }
/// env  = ["CI"]
/// ```
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Profile {
    pub default: Option<String>,
    pub jobs: Option<usize>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub trust: bool,
    /// Var overrides merged into `[vars]` (profile wins on conflict).
    #[serde(default)]
    pub vars: HashMap<String, String>,
}

/// Substitute `{{name}}` placeholders in `s`.
///
/// Resolution order:
/// 1. `[vars]` table in `pbuild.toml`
/// 2. Environment variable with the same name
///
/// Unknown placeholders are left as-is so the error surfaces naturally when
/// the command runs.
///
/// `allow_env`: whether to fall back to environment variables for unknown keys.
/// Should be `false` for `shell = true` rules to prevent shell injection via
/// attacker-controlled env vars.
fn interpolate(vars: &HashMap<String, String>, s: &str, allow_env: bool) -> String {
    let mut out = s.to_string();
    let mut pos = 0;
    while let Some(rel_start) = out[pos..].find("{{") {
        let abs_start = pos + rel_start;
        let Some(rel_end) = out[abs_start..].find("}}") else {
            break;
        };
        let abs_end = abs_start + rel_end;
        let key = &out[abs_start + 2..abs_end];
        // Resolution order: [vars] table → environment (if allowed) → leave as-is.
        let value = if let Some(v) = vars.get(key) {
            v.clone()
        } else if allow_env {
            if let Ok(v) = std::env::var(key) {
                v
            } else {
                pos = abs_end + 2;
                continue;
            }
        } else {
            pos = abs_end + 2;
            continue;
        };
        out.replace_range(abs_start..abs_end + 2, &value);
        pos = abs_start + value.len();
    }
    out
}

fn interpolate_vec(vars: &HashMap<String, String>, v: &[String], allow_env: bool) -> Vec<String> {
    v.iter().map(|s| interpolate(vars, s, allow_env)).collect()
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RawTargetType {
    #[default]
    File,
    Task,
}

#[derive(Debug, Deserialize)]
pub struct RawRule {
    #[serde(rename = "type", default)]
    pub kind: RawTargetType,
    /// Single command shorthand: `command = ["cc", "-o", "app"]`
    #[serde(default)]
    pub command: Vec<String>,
    /// Multi-step commands: `commands = [["step1"], ["step2"]]`
    #[serde(default)]
    pub commands: Vec<Vec<String>>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub output: String,
    /// Path where the compiler will write a Make-style depfile.
    pub depfile: Option<String>,
    /// If true, join each command and run it via `sh -c`.
    #[serde(default)]
    pub shell: bool,
    /// Working directory for the command, relative to pbuild.toml.
    pub dir: Option<String>,
    /// Run `pbuild [target]` in this subdirectory (falls back to `make` if no pbuild.toml).
    pub subdir: Option<String>,
    /// Run `make [target]` in this subdirectory.
    pub makedir: Option<String>,
    /// Short description shown in `--list` output.
    pub description: Option<String>,
    /// Group heading for `--list` output.
    pub group: Option<String>,
    /// Environment variables set only for this rule's commands.
    /// Example: `env = {"CC" = "clang", "CFLAGS" = "-O2"}`
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Pass stdin through to the process (for interactive programs like QEMU).
    #[serde(default)]
    pub tty: bool,
    /// Whether to cache this rule's results. Default true.
    /// Set `cache = false` to always re-run this rule.
    #[serde(default = "default_true")]
    pub cache: bool,
    /// Glob pattern: run the rule once per matching file, substituting
    /// `{{file}}` in commands with each path. Timing is aggregated.
    pub for_each: Option<String>,
    /// How output is displayed: "display" (default), "mute", or "percent".
    #[serde(default)]
    pub progress: String,
    /// Files to download and optionally extract before running commands.
    #[serde(default)]
    pub downloads: Vec<RawDownload>,
    /// Maximum time this rule may run. Accepts "5m", "30s", "1h", or plain seconds.
    /// Overrides `[config] max_time` when set.
    pub max_time: Option<String>,
    /// Number of times to retry on failure (not on timeout). Default 0.
    #[serde(default)]
    pub retry: u32,
    /// Command to run after all retries are exhausted. Empty = no cleanup.
    #[serde(default)]
    pub on_failure: Vec<String>,
}

/// A download step: fetch a URL and optionally extract it.
#[derive(Debug, Deserialize)]
pub struct RawDownload {
    /// URL to fetch.
    pub url: String,
    /// Local directory to extract/place files into.
    pub dest: String,
    /// Archive format: "tar.gz", "tar.xz", "tar.bz2", "tar", "zip", or "none".
    /// Omit to auto-detect from URL extension.
    pub extract: Option<String>,
    /// Strip leading path components when extracting (default 0).
    #[serde(default)]
    pub strip: u32,
}

fn default_true() -> bool {
    true
}

/// Parse a duration string like "5m", "30s", "1h", "1h30m", or a plain integer (seconds).
pub fn parse_duration(s: &str) -> anyhow::Result<std::time::Duration> {
    // Plain integer = seconds.
    if let Ok(n) = s.parse::<u64>() {
        return Ok(std::time::Duration::from_secs(n));
    }
    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();
    for ch in s.chars() {
        match ch {
            '0'..='9' => num_buf.push(ch),
            'h' => {
                let n: u64 = num_buf.parse().map_err(|_| anyhow::anyhow!("invalid duration: {s}"))?;
                total_secs += n * 3600;
                num_buf.clear();
            }
            'm' => {
                let n: u64 = num_buf.parse().map_err(|_| anyhow::anyhow!("invalid duration: {s}"))?;
                total_secs += n * 60;
                num_buf.clear();
            }
            's' => {
                let n: u64 = num_buf.parse().map_err(|_| anyhow::anyhow!("invalid duration: {s}"))?;
                total_secs += n;
                num_buf.clear();
            }
            _ => anyhow::bail!("invalid duration: {s}"),
        }
    }
    if !num_buf.is_empty() {
        anyhow::bail!("invalid duration: {s} (trailing number with no unit)");
    }
    Ok(std::time::Duration::from_secs(total_secs))
}

fn parse_output_mode(s: &str, rule_name: &str) -> Result<OutputMode> {
    match s {
        "" | "display" => Ok(OutputMode::Display),
        "mute" => Ok(OutputMode::Mute),
        "percent" => Ok(OutputMode::Percent),
        other => bail!("rule `{rule_name}`: unknown progress mode `{other}` (expected display, mute, or percent)"),
    }
}

fn parse_ui_config(table: &mut toml::Table) -> Result<crate::ui::UiConfig> {
    let gha = std::env::var("CI").as_deref() == Ok("true");
    let Some(val) = table.remove("ui") else {
        return Ok(crate::ui::UiConfig {
            color: None,
            prefix: None,
            log: None,
            gha,
        });
    };
    let t: toml::Table = val.try_into().context("invalid [ui] section")?;
    let color = t.get("color").and_then(toml::Value::as_bool);
    let prefix = t
        .get("prefix")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    Ok(crate::ui::UiConfig {
        color,
        prefix,
        log: None,
        gha,
    })
}

/// Parse the `[vars]` table, supporting both plain string values and arrays
/// of fallback candidates.
///
/// When a var is an array, pbuild resolves it at load time by checking each
/// candidate in order:
///   1. If the candidate contains a path separator or starts with `.`, treat
///      it as a file path and use it if the file exists.
///   2. Otherwise treat it as a program name and accept it if `which` finds it
///      (or it exists as a file directly).
///
/// The first candidate that resolves wins. If none resolve, the last candidate
/// is used as-is (so the error surfaces naturally when the command runs).
///
/// Example:
/// ```toml
/// [vars]
/// maturin = [".venv/bin/maturin", "maturin"]
/// python  = [".venv/bin/python3", "python3", "python"]
/// ```
fn parse_vars(val: toml::Value) -> anyhow::Result<HashMap<String, String>> {
    let table: toml::Table = val.try_into()?;
    let mut out = HashMap::new();
    for (key, v) in table {
        match v {
            toml::Value::String(s) => {
                out.insert(key, s);
            }
            toml::Value::Array(candidates) => {
                let strings: Vec<String> = candidates
                    .into_iter()
                    .map(|c| {
                        c.try_into::<String>().map_err(|_| {
                            anyhow::anyhow!("[vars] `{key}`: fallback array must contain strings")
                        })
                    })
                    .collect::<anyhow::Result<_>>()?;

                if strings.is_empty() {
                    anyhow::bail!("[vars] `{key}`: fallback array must not be empty");
                }

                let resolved = strings
                    .iter()
                    .find(|candidate| resolve_candidate(candidate))
                    .cloned()
                    .unwrap_or_else(|| strings.last().unwrap().clone());

                out.insert(key, resolved);
            }
            toml::Value::Table(t) => {
                if let Some(eval_val) = t.get("eval") {
                    let cmd = eval_val.as_str().ok_or_else(|| {
                        anyhow::anyhow!("[vars] `{key}`: eval must be a string")
                    })?;
                    let output = std::process::Command::new("sh")
                        .args(["-c", cmd])
                        .output()
                        .with_context(|| format!("[vars] `{key}`: failed to run eval: {cmd}"))?;
                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        anyhow::bail!("[vars] `{key}`: eval failed: {cmd}\n{stderr}");
                    }
                    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    out.insert(key, value);
                } else {
                    anyhow::bail!(
                        "[vars] `{key}`: table var must have an `eval` key"
                    );
                }
            }
            other => anyhow::bail!(
                "[vars] `{key}`: expected string, array, or {{eval = ...}}, got {}",
                other.type_str()
            ),
        }
    }
    Ok(out)
}

/// Return true if `candidate` can be resolved to an executable or existing file.
fn resolve_candidate(candidate: &str) -> bool {
    let path = std::path::Path::new(candidate);
    // Explicit path (relative or absolute) — check if the file exists.
    if candidate.starts_with('.') || candidate.starts_with('/') || candidate.contains('/') {
        return path.exists();
    }
    // Bare program name — check PATH via `which`.
    which_exists(candidate)
}

fn which_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path_var| {
        std::env::split_paths(&path_var).any(|dir| dir.join(name).is_file())
    })
}

/// Merge a named profile into a `BuildFile` in place.
///
/// Returns an error if the profile name is not found.
pub fn apply_profile(bf: &mut BuildFile, name: &str) -> Result<()> {
    let profile = bf
        .config
        .profiles
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no profile named `{name}`"))?;

    if let Some(j) = profile.jobs {
        bf.config.jobs = Some(j);
    }
    if let Some(d) = profile.default {
        bf.config.default = Some(d);
    }
    if profile.trust {
        bf.config.trust = true;
    }
    bf.config.env.extend(profile.env);
    // Profile vars win over base vars.
    for (k, v) in profile.vars {
        bf.vars.insert(k, v);
    }

    Ok(())
}

/// Parse `pbuild.toml` from the current directory.
///
/// The file is a flat TOML table where `[config]` holds build metadata and
/// every other top-level table is treated as a rule.
pub fn load_build_file() -> Result<BuildFile> {
    let src = fs::read_to_string("pbuild.toml").context("could not read pbuild.toml")?;

    let mut table: toml::Table = toml::from_str(&src).context("invalid pbuild.toml")?;

    let config = match table.remove("config") {
        Some(v) => v.try_into().context("invalid [config] section")?,
        None => BuildConfig::default(),
    };

    let ui = parse_ui_config(&mut table)?;

    let vars: HashMap<String, String> = match table.remove("vars") {
        Some(v) => parse_vars(v).context("invalid [vars] section")?,
        None => HashMap::new(),
    };

    let rules = table
        .into_iter()
        .map(|(name, value)| {
            let raw: RawRule = value
                .try_into()
                .with_context(|| format!("invalid rule `{name}`"))?;
            Ok((name, raw))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    Ok(BuildFile {
        config,
        ui,
        vars,
        rules,
    })
}

/// Convert a `BuildFile` into a flat list of `Rule`s.
///
/// Glob patterns in `inputs` are expanded to concrete file paths at this point.
pub fn to_rules(bf: &BuildFile) -> Result<Vec<Rule>> {
    bf.rules
        .iter()
        .map(|(name, raw)| {
            let target = rule_target(name, raw);
            let deps = raw.deps.iter().map(|d| resolve_dep(bf, d)).collect();
            let inputs = expand_globs(&interpolate_vec(&bf.vars, &raw.inputs, true))
                .with_context(|| format!("rule `{name}`: failed to expand inputs"))?;
            // Merge `command` (single) and `commands` (multi-step) into one list.
            // `command` is prepended if both are specified.
            // Env-var interpolation is disabled for shell rules to prevent injection.
            let cmd_env = !raw.shell;
            let mut commands: Vec<Vec<String>> = Vec::new();
            if !raw.command.is_empty() {
                commands.push(interpolate_vec(&bf.vars, &raw.command, cmd_env));
            }
            for cmd in &raw.commands {
                commands.push(interpolate_vec(&bf.vars, cmd, cmd_env));
            }
            if commands.is_empty() {
                anyhow::bail!("rule `{name}` has no command");
            }
            Ok(Rule {
                target,
                deps,
                inputs,
                output: interpolate(&bf.vars, &raw.output, true),
                depfile: raw
                    .depfile
                    .as_deref()
                    .map(|s| interpolate(&bf.vars, s, true)),
                commands,
                shell: raw.shell,
                dir: raw.dir.as_deref().map(|s| interpolate(&bf.vars, s, true)),
                subdir: raw
                    .subdir
                    .as_deref()
                    .map(|s| interpolate(&bf.vars, s, true)),
                makedir: raw
                    .makedir
                    .as_deref()
                    .map(|s| interpolate(&bf.vars, s, true)),
                description: raw.description.clone(),
                group: raw.group.clone(),
                env: raw.env.iter().map(|(k, v)| {
                    (k.clone(), interpolate(&bf.vars, v, true))
                }).collect(),
                tty: raw.tty,
                cache: raw.cache,
                for_each: raw.for_each.as_deref().map(|s| interpolate(&bf.vars, s, true)),
                progress: parse_output_mode(&raw.progress, name)?,
                downloads: raw.downloads.iter().map(|d| Download {
                    url: interpolate(&bf.vars, &d.url, true),
                    dest: interpolate(&bf.vars, &d.dest, true),
                    extract: d.extract.clone(),
                    strip: d.strip,
                }).collect(),
                max_time: {
                    // Rule-level max_time takes precedence over config-level default.
                    let s = raw.max_time.as_deref().or(bf.config.max_time.as_deref());
                    s.map(|s| parse_duration(s))
                        .transpose()
                        .with_context(|| format!("rule `{name}`: invalid max_time"))?
                },
                retry: raw.retry,
                on_failure: interpolate_vec(&bf.vars, &raw.on_failure, !raw.shell),
            })
        })
        .collect()
}

/// Resolve the default or requested target name to a `Target`.
pub fn resolve_target(bf: &BuildFile, name: Option<&str>) -> Result<Target> {
    match name {
        Some(n) => match bf.rules.get(n) {
            Some(raw) => Ok(rule_target(n, raw)),
            None => bail!("no rule for target: {n}"),
        },
        None => match &bf.config.default {
            Some(d) => match bf.rules.get(d.as_str()) {
                Some(raw) => Ok(rule_target(d, raw)),
                None => bail!("default target `{d}` has no rule"),
            },
            None => {
                if bf.rules.len() == 1 {
                    let (name, raw) = bf.rules.iter().next().unwrap();
                    Ok(rule_target(name, raw))
                } else {
                    bail!(
                        "no default target specified and multiple rules exist; \
                         pass a target name on the command line"
                    )
                }
            }
        },
    }
}

/// Build a `Target` from a rule's key and its explicit `type` field.
fn rule_target(name: &str, raw: &RawRule) -> Target {
    match raw.kind {
        RawTargetType::Task => Target::Task(name.to_string()),
        RawTargetType::File => Target::File(name.to_string()),
    }
}

/// Resolve a dependency string to a `Target` by looking it up in the build file.
/// Falls back to `File` if the name isn't a known rule key (e.g. a source file
/// listed as a dep directly).
fn resolve_dep(bf: &BuildFile, name: &str) -> Target {
    match bf.rules.get(name) {
        Some(raw) => rule_target(name, raw),
        None => Target::File(name.to_string()),
    }
}
