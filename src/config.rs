use std::collections::HashMap;
use std::fs;

use anyhow::{bail, Context, Result};
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
/// ["main.o"]
/// command = ["cc", "-c", "main.c", "-o", "main.o"]
/// inputs  = ["main.c"]
/// output  = "main.o"
///
/// [app]
/// command = ["cc", "-o", "app", "main.o"]
/// deps    = ["main.o"]
/// output  = "app"
///
/// [clean]
/// type    = "task"
/// command = ["rm", "-f", "main.o", "app"]
/// ```
///
/// Rules default to `type = "file"`. Set `type = "task"` for phony targets
/// that should always run and are never hashed.
pub struct BuildFile {
    pub config: BuildConfig,
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
    pub command: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub output: String,
    /// Path where the compiler will write a Make-style depfile.
    pub depfile: Option<String>,
}

/// Parse `pbuild.toml` from the current directory.
///
/// The file is a flat TOML table where `[config]` holds build metadata and
/// every other top-level table is treated as a rule.
pub fn load_build_file() -> Result<BuildFile> {
    let src = fs::read_to_string("pbuild.toml")
        .context("could not read pbuild.toml")?;

    let mut table: toml::Table = toml::from_str(&src).context("invalid pbuild.toml")?;

    let config = match table.remove("config") {
        Some(v) => v.try_into().context("invalid [config] section")?,
        None => BuildConfig::default(),
    };

    let rules = table
        .into_iter()
        .map(|(name, value)| {
            let raw: RawRule = value.try_into()
                .with_context(|| format!("invalid rule `{name}`"))?;
            Ok((name, raw))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    Ok(BuildFile { config, rules })
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
            let inputs = expand_globs(&raw.inputs)
                .with_context(|| format!("rule `{name}`: failed to expand inputs"))?;
            Ok(Rule {
                target,
                deps,
                inputs,
                output: raw.output.clone(),
                depfile: raw.depfile.clone(),
                command: raw.command.clone(),
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
