use std::fs;
use std::sync::Mutex;

use pbuild::config::{load_build_file, parse_duration, to_rules};
use tempfile::TempDir;

// `set_current_dir` is process-wide, so parallel tests would race.
static CWD_LOCK: Mutex<()> = Mutex::new(());

fn in_tempdir(f: impl FnOnce(&TempDir)) {
    let _guard = CWD_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    f(&dir);
    std::env::set_current_dir(prev).unwrap();
}

#[test]
fn glob_inputs_expand_to_matching_files() {
    in_tempdir(|dir| {
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.c"), "").unwrap();
        fs::write(dir.path().join("src/b.c"), "").unwrap();
        fs::write(
            dir.path().join("pbuild.toml"),
            r#"
            [app]
            command = ["cc", "-o", "app"]
            inputs  = ["src/*.c"]
            output  = "app"
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        let mut inputs = rules[0].inputs.clone();
        inputs.sort();
        assert_eq!(inputs, ["src/a.c", "src/b.c"]);
    });
}

#[test]
fn literal_input_preserved_when_no_match() {
    in_tempdir(|_dir| {
        fs::write(
            "pbuild.toml",
            r#"
            [app]
            command = ["cc", "-o", "app"]
            inputs  = ["src/missing.c"]
            output  = "app"
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        assert_eq!(rules[0].inputs, ["src/missing.c"]);
    });
}

#[test]
fn vars_substituted_in_command() {
    in_tempdir(|_dir| {
        fs::write(
            "pbuild.toml",
            r#"
            [vars]
            cargo = "cargo"
            profile = "release"

            [build]
            type    = "task"
            command = ["{{cargo}}", "build", "--{{profile}}"]
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        assert_eq!(rules[0].commands[0], ["cargo", "build", "--release"]);
    });
}

#[test]
fn vars_fall_back_to_env() {
    in_tempdir(|_dir| {
        // PATH is always set — use it as a known-present env var.
        fs::write(
            "pbuild.toml",
            r#"
            [build]
            type    = "task"
            command = ["echo", "{{PATH}}"]
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        // If the env fallback works, the placeholder is replaced with the real PATH value.
        let path_val = std::env::var("PATH").unwrap();
        assert_eq!(rules[0].commands[0][1], path_val);
    });
}

#[test]
fn unknown_var_left_as_is() {
    in_tempdir(|_dir| {
        fs::write(
            "pbuild.toml",
            r#"
            [build]
            type    = "task"
            command = ["{{no_such_var}}", "build"]
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        assert_eq!(rules[0].commands[0][0], "{{no_such_var}}");
    });
}

#[test]
fn parse_duration_handles_formats() {
    use std::time::Duration;
    assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
    assert_eq!(parse_duration("1h30m").unwrap(), Duration::from_secs(5400));
    assert_eq!(parse_duration("2h5m10s").unwrap(), Duration::from_secs(7510));
    assert!(parse_duration("bad").is_err());
    assert!(parse_duration("5x").is_err());
}

#[test]
fn max_time_parsed_from_rule() {
    use std::time::Duration;
    in_tempdir(|_dir| {
        fs::write(
            "pbuild.toml",
            r#"
            [test]
            type     = "task"
            command  = ["true"]
            max_time = "2m"
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        assert_eq!(rules[0].max_time, Some(Duration::from_secs(120)));
    });
}

#[test]
fn global_max_time_inherited_by_rules() {
    use std::time::Duration;
    in_tempdir(|_dir| {
        fs::write(
            "pbuild.toml",
            r#"
            [config]
            max_time = "10m"

            [build]
            type    = "task"
            command = ["true"]
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        assert_eq!(rules[0].max_time, Some(Duration::from_secs(600)));
    });
}

#[test]
fn rule_max_time_overrides_global() {
    use std::time::Duration;
    in_tempdir(|_dir| {
        fs::write(
            "pbuild.toml",
            r#"
            [config]
            max_time = "10m"

            [quick]
            type     = "task"
            command  = ["true"]
            max_time = "5s"
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        let rules = to_rules(&bf).unwrap();
        assert_eq!(rules[0].max_time, Some(Duration::from_secs(5)));
    });
}

#[test]
fn invalid_glob_pattern_returns_err() {
    in_tempdir(|_dir| {
        fs::write(
            "pbuild.toml",
            r#"
            [app]
            command = ["cc", "-o", "app"]
            inputs  = ["src/[invalid"]
            output  = "app"
        "#,
        )
        .unwrap();

        let bf = load_build_file().unwrap();
        assert!(to_rules(&bf).is_err());
    });
}
