use std::collections::HashMap;
use std::fs;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::types::{Rule, Target};

/// Raw TOML schema for `pbuild.toml`.
///
/// ```toml
/// [rules.app]
/// command = ["cc", "-o", "app", "main.o"]
/// deps    = ["main.o"]
/// inputs  = ["main.o"]
/// output  = "app"
///
/// [rules.main.o]
/// command = ["cc", "-c", "main.c", "-o", "main.o"]
/// inputs  = ["main.c"]
/// output  = "main.o"
/// ```
///
/// Rule keys that contain a `/` are treated as `File` targets;
/// everything else is a `Task`.
#[derive(Debug, Deserialize)]
pub struct BuildFile {
    /// Optional default target name to build when none is specified on the CLI.
    pub default: Option<String>,
    #[serde(default)]
    pub rules: HashMap<String, RawRule>,
}

#[derive(Debug, Deserialize)]
pub struct RawRule {
    pub command: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub output: String,
}

/// Parse `pbuild.toml` from the current directory.
pub fn load_build_file() -> Result<BuildFile> {
    let src = fs::read_to_string("pbuild.toml")
        .context("could not read pbuild.toml")?;
    toml::from_str(&src).context("invalid pbuild.toml")
}

/// Convert a `BuildFile` into a flat list of `Rule`s.
pub fn to_rules(bf: &BuildFile) -> Result<Vec<Rule>> {
    bf.rules
        .iter()
        .map(|(name, raw)| {
            let target = parse_target(name);
            let deps = raw.deps.iter().map(|d| parse_target(d)).collect();
            Ok(Rule {
                target,
                deps,
                inputs: raw.inputs.clone(),
                output: raw.output.clone(),
                command: raw.command.clone(),
            })
        })
        .collect()
}

/// Resolve the default or requested target name to a `Target`.
pub fn resolve_target(bf: &BuildFile, name: Option<&str>) -> Result<Target> {
    match name {
        Some(n) => Ok(parse_target(n)),
        None => match &bf.default {
            Some(d) => Ok(parse_target(d)),
            None => {
                // Fall back to the only rule if there's exactly one.
                if bf.rules.len() == 1 {
                    Ok(parse_target(bf.rules.keys().next().unwrap()))
                } else {
                    bail!("no default target specified and multiple rules exist;\
                           pass a target name on the command line")
                }
            }
        },
    }
}

/// A target key that looks like a file path (contains `/` or `.`) becomes
/// `Target::File`; otherwise it's `Target::Task`.
fn parse_target(s: &str) -> Target {
    if s.contains('/') || s.contains('.') {
        Target::File(s.to_string())
    } else {
        Target::Task(s.to_string())
    }
}
