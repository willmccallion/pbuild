use pbuild::engine::{execute_plan, Config};
use pbuild::graph::build_plan;
use pbuild::hash::is_dirty;
use pbuild::types::{Rule, Target};
use tempfile::NamedTempFile;

fn mk_task(target: Target, deps: Vec<Target>, command: Vec<&str>) -> Rule {
    Rule {
        target,
        deps,
        inputs: vec![],
        output: String::new(),
        command: command.into_iter().map(ToString::to_string).collect(),
    }
}

fn serial_cfg() -> Config {
    Config { jobs: 1, ..Default::default() }
}

#[test]
fn all_rules_run_on_cold_build() {
    let a = Target::Task("a".into());
    let b = Target::Task("b".into());
    let rules = vec![
        mk_task(a.clone(), vec![], vec!["true"]),
        mk_task(b.clone(), vec![a], vec!["true"]),
    ];
    let plan = build_plan(&rules, &b).unwrap();
    execute_plan(&serial_cfg(), &plan).unwrap();
}

#[test]
fn execution_order_respects_dependencies() {
    let log = NamedTempFile::new().unwrap();
    let log_path = log.path().to_str().unwrap().to_string();

    let a = Target::Task("a".into());
    let b = Target::Task("b".into());
    let c = Target::Task("c".into());

    let append = |name: &str| {
        vec!["sh".to_string(), "-c".to_string(), format!("echo {} >> {}", name, log_path)]
    };

    let rules = vec![
        Rule { target: a.clone(), deps: vec![],        inputs: vec![], output: String::new(), command: append("a") },
        Rule { target: b.clone(), deps: vec![a.clone()], inputs: vec![], output: String::new(), command: append("b") },
        Rule { target: c.clone(), deps: vec![b.clone()], inputs: vec![], output: String::new(), command: append("c") },
    ];

    let plan = build_plan(&rules, &c).unwrap();
    execute_plan(&serial_cfg(), &plan).unwrap();

    let contents = std::fs::read_to_string(log.path()).unwrap();
    let lines: Vec<_> = contents.lines().collect();
    assert_eq!(lines, ["a", "b", "c"]);
}

#[test]
fn rules_with_no_inputs_always_run() {
    let t = Target::Task("always".into());
    let rules = vec![mk_task(t.clone(), vec![], vec!["true"])];
    let plan = build_plan(&rules, &t).unwrap();
    execute_plan(&serial_cfg(), &plan).unwrap();
    execute_plan(&serial_cfg(), &plan).unwrap(); // must run twice
}

#[test]
fn missing_input_file_is_always_dirty() {
    let lf = std::collections::HashMap::from([(
        "/tmp/pbuild-nonexistent-xyz".to_string(),
        "somehash".to_string(),
    )]);
    assert!(is_dirty(&lf, "/tmp/pbuild-nonexistent-xyz").unwrap());
}
