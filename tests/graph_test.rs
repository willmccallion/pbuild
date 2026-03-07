use pbuild::graph::build_plan;
use pbuild::types::{Rule, Target};

fn mk_rule(target: Target, deps: Vec<Target>) -> Rule {
    Rule {
        target,
        deps,
        inputs: vec![],
        output: String::new(),
        command: vec![],
    }
}

#[test]
fn single_rule_no_deps() {
    let rules = vec![mk_rule(Target::Task("all".into()), vec![])];
    let plan = build_plan(&rules, &Target::Task("all".into())).unwrap();
    let targets: Vec<_> = plan.iter().map(|r| &r.target).collect();
    assert_eq!(targets, [&Target::Task("all".into())]);
}

#[test]
fn linear_chain_resolves_in_order() {
    let a = Target::File("a.o".into());
    let b = Target::File("b".into());
    let rules = vec![mk_rule(a.clone(), vec![]), mk_rule(b.clone(), vec![a.clone()])];
    let plan = build_plan(&rules, &b).unwrap();
    let targets: Vec<_> = plan.iter().map(|r| r.target.clone()).collect();
    assert_eq!(targets, [a, b]);
}

#[test]
fn diamond_dep_included_exactly_once() {
    let shared = Target::File("shared.o".into());
    let left   = Target::File("left.o".into());
    let right  = Target::File("right.o".into());
    let root   = Target::File("root".into());
    let rules = vec![
        mk_rule(shared.clone(), vec![]),
        mk_rule(left.clone(),   vec![shared.clone()]),
        mk_rule(right.clone(),  vec![shared.clone()]),
        mk_rule(root.clone(),   vec![left.clone(), right.clone()]),
    ];
    let plan = build_plan(&rules, &root).unwrap();
    let targets: Vec<_> = plan.iter().map(|r| r.target.clone()).collect();

    assert_eq!(targets.iter().filter(|t| **t == shared).count(), 1);

    let idx = |t: &Target| targets.iter().position(|x| x == t).unwrap();
    assert!(idx(&shared) < idx(&left));
    assert!(idx(&shared) < idx(&right));
    assert!(idx(&left)   < idx(&root));
    assert!(idx(&right)  < idx(&root));
}

#[test]
fn missing_dep_returns_err() {
    let rules = vec![mk_rule(Target::File("foo".into()), vec![Target::File("missing.o".into())])];
    let err = build_plan(&rules, &Target::File("foo".into())).unwrap_err();
    assert!(err.contains("missing.o") || err.contains("No rule"), "{err}");
}

#[test]
fn unknown_root_returns_err() {
    let err = build_plan(&[], &Target::Task("ghost".into())).unwrap_err();
    assert!(err.contains("ghost") || err.contains("No rule"), "{err}");
}

#[test]
fn direct_cycle_returns_err() {
    let a = Target::File("a.o".into());
    let b = Target::File("b.o".into());
    let rules = vec![
        mk_rule(a.clone(), vec![b.clone()]),
        mk_rule(b.clone(), vec![a.clone()]),
    ];
    let err = build_plan(&rules, &a).unwrap_err();
    assert!(err.contains("Cycle"), "{err}");
}

#[test]
fn self_referential_returns_err() {
    let a = Target::File("self.o".into());
    let rules = vec![mk_rule(a.clone(), vec![a.clone()])];
    assert!(build_plan(&rules, &a).is_err());
}
