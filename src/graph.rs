use std::collections::{HashMap, HashSet};

use crate::types::{Rule, Target};

/// Given all known rules and a root target, return a topologically sorted
/// list of rules to execute (leaves first, root last), or an error string.
///
/// Errors:
///   * "No rule for target: <t>"  — a dependency has no matching rule
///   * "Cycle detected at: <t>"  — the dependency graph contains a cycle
pub fn build_plan(rules: &[Rule], root: &Target) -> Result<Vec<Rule>, String> {
    let index: HashMap<&Target, &Rule> = rules.iter().map(|r| (&r.target, r)).collect();

    let mut acc: Vec<Rule> = Vec::new();
    let mut visited: HashSet<&Target> = HashSet::new();

    dfs(root, &index, &mut HashSet::new(), &mut visited, &mut acc)?;

    Ok(acc)
}

fn dfs<'a>(
    target: &'a Target,
    index: &HashMap<&'a Target, &'a Rule>,
    on_stack: &mut HashSet<&'a Target>,
    visited: &mut HashSet<&'a Target>,
    acc: &mut Vec<Rule>,
) -> Result<(), String> {
    if visited.contains(target) {
        return Ok(());
    }
    if on_stack.contains(target) {
        return Err(format!("Cycle detected at: {target}"));
    }

    let rule = index
        .get(target)
        .ok_or_else(|| format!("No rule for target: {target}"))?;

    on_stack.insert(target);

    for dep in &rule.deps {
        dfs(dep, index, on_stack, visited, acc)?;
    }

    on_stack.remove(target);
    visited.insert(target);
    acc.push((*rule).clone());

    Ok(())
}
