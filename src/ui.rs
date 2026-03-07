/// Terminal output helpers — colored status lines and build timing.
///
/// Colors are suppressed automatically when stdout is not a TTY
/// (e.g. when piped to a file or CI log capture).
use std::io::IsTerminal as _;

fn colors() -> bool {
    std::io::stdout().is_terminal()
}

// ANSI codes
const GREEN:  &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED:    &str = "\x1b[31m";
const DIM:    &str = "\x1b[2m";
const BOLD:   &str = "\x1b[1m";
const RESET:  &str = "\x1b[0m";

fn green(s: &str)  -> String { if colors() { format!("{GREEN}{s}{RESET}") } else { s.to_string() } }
fn yellow(s: &str) -> String { if colors() { format!("{YELLOW}{s}{RESET}") } else { s.to_string() } }
fn red(s: &str)    -> String { if colors() { format!("{RED}{s}{RESET}") } else { s.to_string() } }
fn dim(s: &str)    -> String { if colors() { format!("{DIM}{s}{RESET}") } else { s.to_string() } }
fn bold(s: &str)   -> String { if colors() { format!("{BOLD}{s}{RESET}") } else { s.to_string() } }

/// Print the command being run: `  + cargo build --release`
pub fn print_command(cmd: &[String]) {
    println!("  {} {}", bold("+"), dim(&cmd.join(" ")));
}

/// Print a skip line (verbose mode): `  [skip] target`
pub fn print_skip(target: &impl std::fmt::Display) {
    println!("  {}", dim(&format!("[skip] {target}")));
}

/// Print a dry-run line: `  (dry) cargo build --release`
pub fn print_dry_run(cmd: &[String]) {
    println!("  {} {}", yellow("(dry)"), cmd.join(" "));
}

/// Print a successful rule completion: `  [done] target  (0.31s)`
pub fn print_done(target: &impl std::fmt::Display, elapsed: std::time::Duration) {
    let secs = elapsed.as_secs_f64();
    let time = dim(&format!("({secs:.2}s)"));
    println!("  {} {target}  {time}", green("[done]"));
}

/// Print a rule failure: `  [FAIL] target`
pub fn print_fail(target: &impl std::fmt::Display) {
    println!("  {} {target}", red("[FAIL]"));
}

/// Print the env-dirty notice.
pub fn print_env_dirty() {
    println!("{}", yellow("[env] tracked environment variable changed — rebuilding all"));
}
