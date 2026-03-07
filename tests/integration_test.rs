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
        Fixture { dir: TempDir::new().unwrap() }
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
    assert!(out.contains("echo built"), "expected command in output, got: {out}");
    assert!(fx.exists("out.txt"));
}

#[test]
fn second_build_skips_unchanged_rule() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    fx.run_ok(&[]);  // cold build
    let out = fx.run_ok(&["--verbose"]);  // nothing changed
    assert!(out.contains("[skip]"), "expected skip on second build, got: {out}");
}

#[test]
fn modifying_input_triggers_rebuild() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    fx.run_ok(&[]);  // cold build

    // Modify the input.
    fs::write(fx.path("src.txt"), "world").unwrap();

    let out = fx.run_ok(&[]);
    assert!(out.contains("echo built"), "expected rebuild after input change, got: {out}");
}

#[test]
fn dry_run_prints_command_without_creating_output() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", SIMPLE_TOML);
    fx.write("src.txt", "hello");

    let out = fx.run_ok(&["--dry-run"]);
    assert!(out.contains("echo built"), "expected command in dry-run output, got: {out}");
    assert!(!fx.exists("out.txt"), "dry-run must not create output files");
}

#[test]
fn list_shows_all_targets() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", r#"
        [config]
        default = "all"

        [all]
        type    = "task"
        command = ["true"]

        ["foo.o"]
        command = ["true"]
        output  = "foo.o"
    "#);

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

    fx.run_ok(&[]);  // build to produce outputs
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
    assert!(!out.status.success(), "expected nonzero exit for unknown target");
}

#[test]
fn dep_chain_builds_in_order() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", r#"
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
    "#);
    fx.write("src.txt", "hello");

    fx.run_ok(&[]);
    let contents = fs::read_to_string(fx.path("final.txt")).unwrap();
    assert_eq!(contents.trim(), "hello");
}

#[test]
fn failed_rule_exits_nonzero() {
    let fx = Fixture::new();
    fx.write("pbuild.toml", r#"
        [config]
        default = "fail"

        [fail]
        type    = "task"
        command = ["false"]
    "#);

    let out = fx.run(&[]);
    assert!(!out.status.success(), "expected nonzero exit when rule fails");
}
