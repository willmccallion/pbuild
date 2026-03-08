use pbuild::engine::{Config, execute_plan};
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
        depfile: None,
        commands: vec![command.into_iter().map(ToString::to_string).collect()],
        shell: false,
        dir: None,
        subdir: None,
        makedir: None,
        description: None,
        group: None,
        env: std::collections::HashMap::new(),
        tty: false,
        cache: true,
        for_each: None,
        progress: pbuild::types::OutputMode::Display,
        downloads: vec![],
        max_time: None,
        retry: 0,
    }
}

fn serial_cfg() -> Config {
    Config {
        jobs: 1,
        ..Default::default()
    }
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
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo {} >> {}", name, log_path),
        ]
    };

    let rules = vec![
        Rule {
            target: a.clone(),
            deps: vec![],
            inputs: vec![],
            output: String::new(),
            depfile: None,
            commands: vec![append("a")],
            shell: false,
            dir: None,
            subdir: None,
            makedir: None,
            description: None,
            group: None,
            env: std::collections::HashMap::new(),
            tty: false,
            cache: true,
            for_each: None,
            progress: pbuild::types::OutputMode::Display,
            downloads: vec![],
            max_time: None,
            retry: 0,
        },
        Rule {
            target: b.clone(),
            deps: vec![a.clone()],
            inputs: vec![],
            output: String::new(),
            depfile: None,
            commands: vec![append("b")],
            shell: false,
            dir: None,
            subdir: None,
            makedir: None,
            description: None,
            group: None,
            env: std::collections::HashMap::new(),
            tty: false,
            cache: true,
            for_each: None,
            progress: pbuild::types::OutputMode::Display,
            downloads: vec![],
            max_time: None,
            retry: 0,
        },
        Rule {
            target: c.clone(),
            deps: vec![b.clone()],
            inputs: vec![],
            output: String::new(),
            depfile: None,
            commands: vec![append("c")],
            shell: false,
            dir: None,
            subdir: None,
            makedir: None,
            description: None,
            group: None,
            env: std::collections::HashMap::new(),
            tty: false,
            cache: true,
            for_each: None,
            progress: pbuild::types::OutputMode::Display,
            downloads: vec![],
            max_time: None,
            retry: 0,
        },
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
fn keep_going_runs_independent_rules_after_failure() {
    let log = NamedTempFile::new().unwrap();
    let log_path = log.path().to_str().unwrap().to_string();

    // Graph: `fail` and `ok` are both independent leaves; `root` depends on both.
    //
    //   fail  ok
    //     \  /
    //     root
    //
    // Without -k, `root` would never run and neither would `ok` (if `fail` is first).
    // With -k, `ok` must run even though `fail` fails.
    let fail = Target::Task("fail".into());
    let ok = Target::Task("ok".into());
    let root = Target::Task("root".into());

    let rules = vec![
        mk_task(fail.clone(), vec![], vec!["false"]),
        mk_task(
            ok.clone(),
            vec![],
            vec!["sh", "-c", &format!("echo ok >> {log_path}")],
        ),
        mk_task(root.clone(), vec![fail.clone(), ok.clone()], vec!["true"]),
    ];

    let plan = build_plan(&rules, &root).unwrap();
    let cfg = Config {
        jobs: 1,
        keep_going: true,
        ..Default::default()
    };
    let err = execute_plan(&cfg, &plan).unwrap_err();

    // Build must have failed overall.
    assert!(err.to_string().contains("1 rule(s) failed"), "{err}");

    // But `ok` must have run.
    let contents = std::fs::read_to_string(log.path()).unwrap();
    assert!(contents.contains("ok"), "expected `ok` to have run");
}

#[test]
fn retry_succeeds_after_initial_failure() {
    // Write a counter file; the rule fails until it has been attempted enough times.
    let counter = tempfile::NamedTempFile::new().unwrap();
    let counter_path = counter.path().to_str().unwrap().to_string();
    // Script: increment counter, fail on first attempt (count == 1), succeed on second.
    let script = format!(
        "c=$(cat {p} 2>/dev/null || echo 0); echo $((c+1)) > {p}; [ $((c+1)) -ge 2 ]",
        p = counter_path
    );
    let t = Target::Task("flaky".into());
    let mut rule = mk_task(t.clone(), vec![], vec!["sh", "-c", &script]);
    rule.retry = 1; // 1 retry = 2 total attempts
    let rules = vec![rule];
    let plan = build_plan(&rules, &t).unwrap();
    execute_plan(&serial_cfg(), &plan).unwrap();
    // Verify the rule ran twice (counter == 2).
    let count: u32 = std::fs::read_to_string(counter.path())
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn retry_exhausted_returns_error() {
    let t = Target::Task("always-fail".into());
    let mut rule = mk_task(t.clone(), vec![], vec!["false"]);
    rule.retry = 2; // 2 retries = 3 total attempts, all fail
    let plan = build_plan(&[rule], &t).unwrap();
    assert!(execute_plan(&serial_cfg(), &plan).is_err());
}

#[test]
fn missing_input_file_is_always_dirty() {
    let lf = std::collections::HashMap::from([(
        "/tmp/pbuild-nonexistent-xyz".to_string(),
        "somehash".to_string(),
    )]);
    assert!(is_dirty(&lf, "/tmp/pbuild-nonexistent-xyz").unwrap());
}
