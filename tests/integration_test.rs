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
fn shell_true_enables_shell_features() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", r#"
        [config]
        default = "run"

        [run]
        type    = "task"
        shell   = true
        command = ["echo hello > out.txt && echo world >> out.txt"]
    "#);

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
