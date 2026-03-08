use std::collections::{HashMap, HashSet};

use crate::types::{Rule, Target};

/// Print an ASCII representation of the dependency graph rooted at `root`.
///
/// Example output:
/// ```text
/// build
/// ├── rust
/// └── python
/// ```
pub fn print_graph(rules: &[Rule], root: &Target) {
    let index: HashMap<&Target, &Rule> = rules.iter().map(|r| (&r.target, r)).collect();
    // Print the root with no connector, then recurse into its deps.
    println!("{root}");
    if let Some(rule) = index.get(root) {
        let deps = &rule.deps;
        for (i, dep) in deps.iter().enumerate() {
            let last = i == deps.len() - 1;
            print_node(dep, &index, "", last);
        }
    }
}

fn print_node(
    target: &Target,
    index: &HashMap<&Target, &Rule>,
    prefix: &str,
    is_last: bool,
) {
    let connector = if is_last { "└── " } else { "├── " };
    println!("{prefix}{connector}{target}");

    let child_prefix = if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };

    if let Some(rule) = index.get(target) {
        let deps = &rule.deps;
        for (i, dep) in deps.iter().enumerate() {
            let last = i == deps.len() - 1;
            print_node(dep, index, &child_prefix, last);
        }
    }
}

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
