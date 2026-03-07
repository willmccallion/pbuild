use std::collections::HashMap;
use std::fs;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::types::{Rule, Target};

/// Expand a list of glob patterns into concrete file paths.
/// Exposed so callers outside `config` (e.g. `pbuild why`) can reuse it.
pub fn expand_inputs(patterns: &[String]) -> Result<Vec<String>> {
    expand_globs(patterns)
}

fn expand_globs(patterns: &[String]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for pattern in patterns {
        let matches: Vec<_> = glob::glob(pattern)
            .with_context(|| format!("invalid glob pattern: {pattern}"))?
            .collect::<Result<_, _>>()
            .with_context(|| format!("error reading glob pattern: {pattern}"))?;

        if matches.is_empty() {
            // Keep the literal string so the engine can report a meaningful
            // "missing input" error rather than silently skipping it.
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
    /// Short description shown in `--list` output.
    pub description: Option<String>,
    /// Group heading for `--list` output.
    pub group: Option<String>,
}

fn parse_ui_config(table: &mut toml::Table) -> Result<crate::ui::UiConfig> {
    let Some(val) = table.remove("ui") else {
        return Ok(crate::ui::UiConfig { color: None, prefix: None });
    };
    let t: toml::Table = val.try_into().context("invalid [ui] section")?;
    let color = t.get("color").and_then(|v| v.as_bool());
    let prefix = t.get("prefix").and_then(|v| v.as_str()).map(ToString::to_string);
    Ok(crate::ui::UiConfig { color, prefix })
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
        Some(v) => v.try_into().context("invalid [vars] section")?,
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
                depfile: raw.depfile.as_deref().map(|s| interpolate(&bf.vars, s, true)),
                commands,
                shell: raw.shell,
                description: raw.description.clone(),
                group: raw.group.clone(),
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
