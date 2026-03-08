use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

fn pbuild_bin() -> std::path::PathBuf {
    // Use the debug binary built by cargo test's own build step.
    let mut path = std::env::current_exe().unwrap();
    // current_exe is something like target/debug/deps/integration_test-<hash>
    // Walk up to target/debug/
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("pbuild");
    path
}

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            dir: TempDir::new().unwrap(),
        }
    }

    fn write(&self, name: &str, contents: &str) {
        let path = self.dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(pbuild_bin())
            .args(args)
            .current_dir(self.dir.path())
            .output()
            .expect("failed to run pbuild")
    }

    fn run_ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            out.status.success(),
            "pbuild failed\nstdout: {stdout}\nstderr: {stderr}"
        );
        stdout
    }

    fn exists(&self, name: &str) -> bool {
        self.dir.path().join(name).exists()
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }
}

// Minimal pbuild.toml that echoes a sentinel string as its "build".
const SIMPLE_TOML: &str = r#"
[config]
default = "out.txt"

["out.txt"]
command = ["sh", "-c", "echo built > out.txt"]
inputs  = ["src.txt"]
output  = "out.txt"
"#;

#[test]
fn cold_build_runs_rule() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    let out = fx.run_ok(&[]);
    assert!(
        out.contains("echo built"),
        "expected command in output, got: {out}"
    );
    assert!(fx.exists("out.txt"));
}

#[test]
fn second_build_skips_unchanged_rule() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    fx.run_ok(&[]); // cold build
    let out = fx.run_ok(&["--verbose"]); // nothing changed
    assert!(
        out.contains("–"),
        "expected skip on second build, got: {out}"
    );
}

#[test]
fn modifying_input_triggers_rebuild() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    fx.run_ok(&[]); // cold build

    // Modify the input.
    fs::write(fx.path("src.txt"), "world").unwrap();

    let out = fx.run_ok(&[]);
    assert!(
        out.contains("echo built"),
        "expected rebuild after input change, got: {out}"
    );
}

#[test]
fn dry_run_prints_command_without_creating_output() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    let out = fx.run_ok(&["--dry-run"]);
    assert!(
        out.contains("echo built"),
        "expected command in dry-run output, got: {out}"
    );
    assert!(
        !fx.exists("out.txt"),
        "dry-run must not create output files"
    );
}

#[test]
fn list_shows_groups_and_descriptions() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "build"

        [build]
        group       = "Build"
        description = "Compile everything"
        type        = "task"
        command     = ["true"]

        [test]
        group       = "Quality"
        description = "Run tests"
        type        = "task"
        command     = ["true"]

        [clean]
        type    = "task"
        command = ["true"]
    "#,
    );

    let out = fx.run_ok(&["--list"]);
    assert!(out.contains("Build"), "expected group header");
    assert!(out.contains("Compile everything"), "expected description");
    assert!(out.contains("(default)"), "expected default marker");
    assert!(out.contains("Quality"), "expected Quality group");
    assert!(out.contains("Other"), "expected ungrouped under Other");
    assert!(out.contains("clean"), "expected ungrouped rule");
}

#[test]
fn list_shows_all_targets() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "all"

        [all]
        type    = "task"
        command = ["true"]

        ["foo.o"]
        command = ["true"]
        output  = "foo.o"
    "#,
    );

    let out = fx.run_ok(&["--list"]);
    assert!(out.contains("all"), "expected `all` in list output");
    assert!(out.contains("foo.o"), "expected `foo.o` in list output");
    assert!(out.contains("(default)"), "expected default marker");
}

#[test]
fn clean_removes_output_and_lock_file() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    fx.run_ok(&[]); // build to produce outputs
    assert!(fx.exists("out.txt"));
    assert!(fx.exists(".pbuild.lock"));

    fx.run_ok(&["clean"]);
    assert!(!fx.exists("out.txt"), "clean should remove output");
    assert!(!fx.exists(".pbuild.lock"), "clean should remove lock file");
}

#[test]
fn unknown_target_exits_nonzero() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    let out = fx.run(&["ghost"]);
    assert!(
        !out.status.success(),
        "expected nonzero exit for unknown target"
    );
}

#[test]
fn dep_chain_builds_in_order() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "final.txt"

        ["mid.txt"]
        command = ["sh", "-c", "cat src.txt > mid.txt"]
        inputs  = ["src.txt"]
        output  = "mid.txt"

        ["final.txt"]
        command = ["sh", "-c", "cat mid.txt > final.txt"]
        deps    = ["mid.txt"]
        inputs  = ["mid.txt"]
        output  = "final.txt"
    "#,
    );
    fx.write("src.txt", "hello");

    fx.run_ok(&[]);
    let contents = fs::read_to_string(fx.path("final.txt")).unwrap();
    assert_eq!(contents.trim(), "hello");
}

#[test]
fn failed_rule_exits_nonzero() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "fail"

        [fail]
        type    = "task"
        command = ["false"]
    "#,
    );

    let out = fx.run(&[]);
    assert!(
        !out.status.success(),
        "expected nonzero exit when rule fails"
    );
}

#[test]
fn depfile_discovered_inputs_trigger_rebuild() {
    let fx = Fixture::new();

    // A script that produces the output AND writes a depfile listing a header.
    // pbuild injects -MF automatically, so we read it from the last two args.
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "out.o"

        ["out.o"]
        command = ["sh", "build.sh"]
        inputs  = ["src.c"]
        output  = "out.o"
        depfile = "out.d"
    "#,
    );
    fx.write("src.c", "// source");
    fx.write("header.h", "// header v1");
    // build.sh simulates a compiler: writes the output and a depfile.
    // pbuild appends `-MF out.d` to the command, so $3/$4 are -MF and out.d.
    fx.write(
        "build.sh",
        "touch out.o && echo \"out.o: src.c header.h\" > $2\n",
    );

    // Cold build — discovers header.h via depfile.
    fx.run_ok(&[]);

    // Second build — nothing changed, should skip.
    let out = fx.run_ok(&["--verbose"]);
    assert!(
        out.contains("–"),
        "expected skip when nothing changed: {out}"
    );

    // Modify the discovered header — should trigger a rebuild.
    fs::write(fx.path("header.h"), "// header v2").unwrap();
    let out = fx.run_ok(&[]);
    assert!(
        out.contains("build.sh"),
        "expected rebuild after header change: {out}"
    );
}

#[test]
fn dangerous_command_blocked_without_trust() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "install"

        [install]
        type    = "task"
        command = ["sudo", "cp", "app", "/usr/bin/app"]
    "#,
    );

    let out = fx.run(&[]);
    assert!(
        !out.status.success(),
        "expected nonzero exit for dangerous command"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsafe"),
        "expected unsafe warning in stderr"
    );
    assert!(
        stderr.contains("--trust"),
        "expected --trust hint in stderr"
    );
}

#[test]
fn dangerous_command_allowed_with_trust_flag() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "run"

        [run]
        type    = "task"
        command = ["sudo", "--version"]
    "#,
    );

    // --trust bypasses the check; sudo --version exits 0 without a password.
    let out = fx.run(&["--trust"]);
    assert!(out.status.success(), "expected success with --trust");
}

#[test]
fn dangerous_command_allowed_with_config_trust() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "run"
        trust   = true

        [run]
        type    = "task"
        command = ["sudo", "--version"]
    "#,
    );

    let out = fx.run(&[]);
    assert!(
        out.status.success(),
        "expected success with config trust = true"
    );
}

#[test]
fn status_shows_dirty_and_clean() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    // Before build: dirty.
    let out = fx.run_ok(&["status"]);
    assert!(
        out.contains("dirty"),
        "expected dirty before build, got: {out}"
    );

    // Build.
    fx.run_ok(&[]);

    // After build: clean.
    let out = fx.run_ok(&["status"]);
    assert!(
        out.contains("clean"),
        "expected clean after build, got: {out}"
    );

    // Modify input: dirty again.
    fs::write(fx.path("src.txt"), "world").unwrap();
    let out = fx.run_ok(&["status"]);
    assert!(
        out.contains("dirty"),
        "expected dirty after input change, got: {out}"
    );
}

#[test]
fn only_flag_skips_deps() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "final.txt"

        ["mid.txt"]
        command = ["sh", "-c", "echo mid > mid.txt"]
        inputs  = ["src.txt"]
        output  = "mid.txt"

        ["final.txt"]
        command = ["sh", "-c", "echo final > final.txt"]
        deps    = ["mid.txt"]
        inputs  = ["mid.txt"]
        output  = "final.txt"
    "#,
    );
    fx.write("src.txt", "hello");

    // --only final.txt should run without building mid.txt first.
    fx.run_ok(&["--only", "final.txt"]);
    assert!(fx.exists("final.txt"), "expected final.txt to be built");
    assert!(!fx.exists("mid.txt"), "--only must not build deps");
}

#[test]
fn dir_field_sets_working_directory() {
    let fx = Fixture::new();
    fx.write("sub/pbuild.toml", ""); // create subdirectory
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "run"

        [run]
        type    = "task"
        dir     = "sub"
        command = ["sh", "-c", "pwd > ../out.txt"]
    "#,
    );

    fx.run_ok(&[]);
    let out = fs::read_to_string(fx.path("out.txt")).unwrap();
    assert!(
        out.trim().ends_with("/sub"),
        "expected command to run in sub/, got: {out}"
    );
}

#[test]
fn shell_true_enables_shell_features() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "run"

        [run]
        type    = "task"
        shell   = true
        command = ["echo hello > out.txt && echo world >> out.txt"]
    "#,
    );

    fx.run_ok(&[]);
    let out = fs::read_to_string(fx.path("out.txt")).unwrap();
    assert_eq!(out.trim(), "hello\nworld");
}

#[test]
fn multi_step_commands_run_in_order() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "setup"

        [setup]
        type     = "task"
        commands = [
            ["sh", "-c", "echo step1 >> log.txt"],
            ["sh", "-c", "echo step2 >> log.txt"],
            ["sh", "-c", "echo step3 >> log.txt"],
        ]
    "#,
    );

    fx.run_ok(&[]);

    let log = fs::read_to_string(fx.path("log.txt")).unwrap();
    assert_eq!(log.trim(), "step1\nstep2\nstep3");
}

#[test]
fn multi_step_fails_on_first_error() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "run"

        [run]
        type     = "task"
        commands = [
            ["sh", "-c", "echo before >> log.txt"],
            ["false"],
            ["sh", "-c", "echo after >> log.txt"],
        ]
    "#,
    );

    let out = fx.run(&[]);
    assert!(!out.status.success(), "expected failure when a step fails");
    // The third step must not have run.
    let log = fs::read_to_string(fx.path("log.txt")).unwrap_or_default();
    assert!(!log.contains("after"), "steps after a failure must not run");
}

#[test]
fn vars_substituted_in_command() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "greet"

        [vars]
        greeting = "hello"

        [greet]
        type    = "task"
        command = ["sh", "-c", "echo {{greeting}}"]
    "#,
    );

    let out = fx.run_ok(&[]);
    assert!(
        out.contains("hello"),
        "expected var substitution in output, got: {out}"
    );
}

#[test]
fn log_flag_writes_output_to_file() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    let log_path = fx.path("build.log");
    let log_str = log_path.to_str().unwrap();
    fx.run_ok(&["--log", log_str]);

    let log = fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("out.txt"),
        "expected target name in log: {log}"
    );
    assert!(log.contains("echo built"), "expected command in log: {log}");
    assert!(
        !log.contains("\x1b["),
        "log must not contain ANSI escape codes: {log}"
    );
}

#[test]
fn completion_fish_is_valid_text() {
    // Run from a temp dir with no pbuild.toml — completion must not require one.
    let fx = Fixture::new();
    let out = Command::new(pbuild_bin())
        .args(["--completion", "fish"])
        .current_dir(fx.dir.path())
        .output()
        .expect("failed to run pbuild");

    assert!(
        out.status.success(),
        "expected zero exit for --completion fish"
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Must not write to stderr.
    assert!(
        stderr.is_empty(),
        "completion must not write to stderr: {stderr}"
    );

    // Must be non-empty fish script.
    assert!(
        stdout.contains("complete -c pbuild"),
        "expected fish complete directives: {stdout}"
    );
    assert!(
        stdout.contains("__pbuild_targets"),
        "expected target helper function: {stdout}"
    );

    // Must not contain anything that looks like a shell escape or injection.
    assert!(
        !stdout.contains("$(rm"),
        "must not contain dangerous subshell"
    );
    assert!(!stdout.contains("eval "), "must not contain eval");

    // Must not write any files to the temp dir.
    assert!(
        fs::read_dir(fx.dir.path()).unwrap().count() == 0,
        "completion must not create any files"
    );
}

#[test]
fn completion_bash_is_valid_text() {
    let fx = Fixture::new();
    let out = Command::new(pbuild_bin())
        .args(["--completion", "bash"])
        .current_dir(fx.dir.path())
        .output()
        .expect("failed to run pbuild");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("_pbuild_complete"),
        "expected bash function: {stdout}"
    );
    assert!(
        stdout.contains("complete -F _pbuild_complete pbuild"),
        "expected complete directive: {stdout}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).is_empty(),
        "no stderr expected"
    );
    assert!(
        fs::read_dir(fx.dir.path()).unwrap().count() == 0,
        "must not create files"
    );
}

#[test]
fn completion_zsh_is_valid_text() {
    let fx = Fixture::new();
    let out = Command::new(pbuild_bin())
        .args(["--completion", "zsh"])
        .current_dir(fx.dir.path())
        .output()
        .expect("failed to run pbuild");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("#compdef pbuild"),
        "expected zsh compdef: {stdout}"
    );
    assert!(
        stdout.contains("_pbuild"),
        "expected zsh function: {stdout}"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).is_empty(),
        "no stderr expected"
    );
    assert!(
        fs::read_dir(fx.dir.path()).unwrap().count() == 0,
        "must not create files"
    );
}

#[test]
fn completion_unknown_shell_fails() {
    let fx = Fixture::new();
    let out = Command::new(pbuild_bin())
        .args(["--completion", "powershell"])
        .current_dir(fx.dir.path())
        .output()
        .expect("failed to run pbuild");

    assert!(
        !out.status.success(),
        "expected nonzero exit for unknown shell"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("powershell"),
        "expected shell name in error: {stderr}"
    );
}

#[test]
fn depfile_mf_flag_injected_automatically() {
    let fx = Fixture::new();

    // Verify that pbuild injects -MF <path> by checking the args the script receives.
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "out.o"

        ["out.o"]
        command = ["sh", "build.sh"]
        inputs  = ["src.c"]
        output  = "out.o"
        depfile = "out.d"
    "#,
    );
    fx.write("src.c", "// source");
    // Record all args to a file so we can inspect them.
    fx.write(
        "build.sh",
        "echo \"$@\" > args.txt && touch out.o && echo 'out.o: src.c' > $2\n",
    );

    fx.run_ok(&[]);

    let args = fs::read_to_string(fx.path("args.txt")).unwrap();
    assert!(
        args.contains("-MF"),
        "expected -MF in injected args: {args}"
    );
    assert!(
        args.contains("out.d"),
        "expected depfile path in injected args: {args}"
    );
}

#[test]
fn doctor_passes_on_valid_config() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [build]
        type    = "task"
        command = ["true"]
    "#,
    );
    let out = fx.run_ok(&["doctor"]);
    assert!(out.contains("All checks passed") || out.contains("✓"), "got: {out}");
}

#[test]
fn doctor_fails_on_missing_dep() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [build]
        type    = "task"
        command = ["true"]
        deps    = ["nonexistent"]
    "#,
    );
    let out = fx.run(&["doctor"]);
    assert!(!out.status.success(), "expected doctor to fail");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("nonexistent"), "got: {stdout}");
}

#[test]
fn doctor_fails_on_duplicate_output() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [a]
        command = ["true"]
        output  = "same.o"

        [b]
        command = ["true"]
        output  = "same.o"
    "#,
    );
    let out = fx.run(&["doctor"]);
    assert!(!out.status.success(), "expected doctor to fail on duplicate output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("same.o"), "got: {stdout}");
}

#[test]
fn explain_shows_expanded_commands() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [vars]
        tool = "cargo"

        [build]
        type    = "task"
        command = ["{{tool}}", "build", "--release"]
        env     = { RUST_LOG = "debug" }
        dir     = "subdir"
        max_time = "5m"
        retry   = 2
    "#,
    );

    let out = fx.run_ok(&["--explain", "build"]);
    assert!(out.contains("cargo"), "var not expanded: {out}");
    assert!(out.contains("RUST_LOG"), "env not shown: {out}");
    assert!(out.contains("subdir"), "dir not shown: {out}");
    assert!(out.contains("5m"), "max_time not shown: {out}");
    assert!(out.contains("retry"), "retry not shown: {out}");
}

#[test]
fn explain_shows_shell_wrapping() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [deploy]
        type    = "task"
        shell   = true
        command = ["cp dist/* /tmp/out && echo done"]
    "#,
    );

    let out = fx.run_ok(&["--explain", "deploy"]);
    assert!(out.contains("sh -c"), "shell wrapping not shown: {out}");
}

#[test]
fn status_json_output() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [config]
        default = "build"

        [build]
        type    = "task"
        command = ["true"]
    "#,
    );
    let out = fx.run_ok(&["status", "--json"]);
    assert!(out.contains("\"target\""), "expected JSON target field: {out}");
    assert!(out.contains("\"state\""), "expected JSON state field: {out}");
    assert!(out.starts_with('['), "expected JSON array: {out}");
}

#[test]
fn graph_json_output() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [a]
        type    = "task"
        command = ["true"]

        [b]
        type    = "task"
        command = ["true"]
        deps    = ["a"]
    "#,
    );
    let out = fx.run_ok(&["graph", "--json", "b"]);
    assert!(out.contains("\"target\""), "expected JSON: {out}");
    assert!(out.contains("\"deps\""), "expected deps field: {out}");
    assert!(out.starts_with('['), "expected JSON array: {out}");
}

#[test]
fn why_json_output() {
    let fx = Fixture::new();
    fx.write(
        "pbuild.toml",
        r#"
        [build]
        type    = "task"
        command = ["true"]
    "#,
    );
    let out = fx.run_ok(&["why", "--json", "build"]);
    assert!(out.contains("\"target\""), "expected JSON target: {out}");
    assert!(out.contains("\"reason\""), "expected JSON reason: {out}");
    assert!(out.starts_with('{'), "expected JSON object: {out}");
}
