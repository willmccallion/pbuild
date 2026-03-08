/// Terminal output helpers — colored status lines and build timing.
///
/// Colors are suppressed automatically when stdout is not a TTY
/// (e.g. when piped to a file or CI log capture), unless overridden
/// via `[ui] color` in `pbuild.toml`.
use std::io::IsTerminal as _;
use std::io::Write as _;
use std::sync::{Arc, Mutex};

/// Optional display tweaks from `[ui]` in `pbuild.toml`.
///
/// ```toml
/// [ui]
/// color  = true   # force color on/off; default: auto-detect TTY
/// prefix = "›"    # symbol printed before each target name; default "›"
/// ```
#[derive(Debug, Clone)]
pub struct UiConfig {
    /// Force color on (`Some(true)`) or off (`Some(false)`).
    /// `None` means auto-detect from TTY.
    pub color: Option<bool>,
    /// Symbol printed before a target name when it starts running.
    pub prefix: Option<String>,
    /// Optional log file — pbuild's own output lines are tee'd here (no ANSI codes).
    pub log: Option<Arc<Mutex<std::fs::File>>>,
}

impl UiConfig {
    fn colors(&self) -> bool {
        self.color
            .unwrap_or_else(|| std::io::stdout().is_terminal())
    }

    fn prefix(&self) -> &str {
        self.prefix.as_deref().unwrap_or("›")
    }

    fn c<'a>(&self, code: &'static str, s: &'a str) -> std::borrow::Cow<'a, str> {
        if self.colors() {
            format!("{code}{s}\x1b[0m").into()
        } else {
            s.into()
        }
    }

    fn green<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        self.c("\x1b[32m", s)
    }
    fn yellow<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        self.c("\x1b[33m", s)
    }
    fn red<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        self.c("\x1b[31m", s)
    }
    fn dim<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        self.c("\x1b[2m", s)
    }
    fn bold<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        self.c("\x1b[1m", s)
    }

    fn log(&self, line: &str) {
        if let Some(f) = &self.log {
            let mut f = f.lock().unwrap();
            let _ = writeln!(f, "{line}");
        }
    }

    /// `› build`  — printed when a rule starts.
    pub fn print_start(&self, target: &impl std::fmt::Display) {
        println!("{} {target}", self.bold(self.prefix()));
        self.log(&format!("{} {target}", self.prefix()));
    }

    /// `    $ cargo build --release`  — each command within a rule.
    pub fn print_command(&self, cmd: &[String]) {
        println!("    {} {}", self.dim("$"), self.dim(&cmd.join(" ")));
        self.log(&format!("    $ {}", cmd.join(" ")));
    }

    /// `– build`  (dim, verbose only)
    pub fn print_skip(&self, target: &impl std::fmt::Display) {
        println!("{} {target}", self.dim("–"));
        self.log(&format!("– {target}"));
    }

    /// `Nothing to do — all targets are up to date.`
    pub fn print_up_to_date(&self) {
        println!("{}", self.dim("Nothing to do — all targets are up to date."));
    }

    /// `  → build  changed: src/main.rs`  (verbose, before a rule runs)
    pub fn print_dirty_reason(&self, target: &impl std::fmt::Display, reason: &str) {
        println!("  {} {target}  {}", self.yellow("→"), self.dim(reason));
        self.log(&format!("  → {target}  {reason}"));
    }

    /// `    (dry) cargo build --release`
    pub fn print_dry_run(&self, cmd: &[String]) {
        println!("    {} {}", self.yellow("dry"), self.dim(&cmd.join(" ")));
        self.log(&format!("    dry {}", cmd.join(" ")));
    }

    /// `  ✓ build  0.31s`
    pub fn print_done(&self, target: &impl std::fmt::Display, elapsed: std::time::Duration) {
        let secs = elapsed.as_secs_f64();
        let time_str = format!("{secs:.2}s");
        let time = self.dim(&time_str);
        println!("  {} {target}  {time}", self.green("✓"));
        self.log(&format!("  ✓ {target}  {secs:.2}s"));
    }

    /// `  ✓ bench  (100 files)  1.23s`
    pub fn print_done_count(
        &self,
        target: &impl std::fmt::Display,
        count: usize,
        elapsed: std::time::Duration,
    ) {
        let secs = elapsed.as_secs_f64();
        let time_str = format!("{secs:.2}s");
        let count_str = format!("({count} files)");
        println!(
            "  {} {target}  {}  {}",
            self.green("✓"),
            self.dim(&count_str),
            self.dim(&time_str)
        );
        self.log(&format!("  ✓ {target}  ({count} files)  {secs:.2}s"));
    }

    /// `    [42/1000] bench-uf50` — overwritten in place with `\r`.
    /// Suppressed when stdout is not a TTY (piped output).
    pub fn print_progress(&self, target: &impl std::fmt::Display, current: usize, total: usize) {
        if !std::io::stdout().is_terminal() {
            return;
        }
        let pct = current * 100 / total;
        let msg = format!("    {} {target}", self.dim(&format!("[{current}/{total}] {pct}%")));
        print!("\r{msg}");
        let _ = std::io::stdout().flush();
    }

    /// Clear the progress line. No-op when stdout is not a TTY.
    pub fn clear_progress(&self) {
        if !std::io::stdout().is_terminal() {
            return;
        }
        print!("\r{}\r", " ".repeat(80));
        let _ = std::io::stdout().flush();
    }

    /// `    ↓ https://example.com/foo.tar.gz → bench/foo`
    pub fn print_download(&self, url: &str, dest: &str) {
        println!("    {} {} {} {}", self.dim("↓"), self.dim(url), self.dim("→"), dest);
        self.log(&format!("    ↓ {url} → {dest}"));
    }

    /// `  ✗ build`
    pub fn print_fail(&self, target: &impl std::fmt::Display) {
        println!("  {} {target}", self.red("✗"));
        self.log(&format!("  ✗ {target}"));
    }

    /// `    ~ cleanup command`  — shown before running on_failure command.
    pub fn print_on_failure_cmd(&self, cmd: &[String]) {
        println!("    {} {}", self.dim("~"), self.dim(&cmd.join(" ")));
        self.log(&format!("    ~ {}", cmd.join(" ")));
    }

    /// `  ↻ fetch  (attempt 2/3)`
    pub fn print_retry(&self, target: &impl std::fmt::Display, attempt: u32, total: u32) {
        let msg = format!("(attempt {attempt}/{total})");
        println!("  {} {target}  {}", self.yellow("↻"), self.dim(&msg));
        self.log(&format!("  ↻ {target}  (attempt {attempt}/{total})"));
    }

    /// `  ✗ test  timed out after 5m`
    pub fn print_timeout(&self, target: &impl std::fmt::Display, limit: std::time::Duration) {
        let secs = limit.as_secs();
        let limit_str = if secs % 3600 == 0 {
            format!("{}h", secs / 3600)
        } else if secs % 60 == 0 {
            format!("{}m", secs / 60)
        } else {
            format!("{secs}s")
        };
        println!("  {} {target}  timed out after {limit_str}", self.red("✗"));
        self.log(&format!("  ✗ {target}  timed out after {limit_str}"));
    }

    /// Flush captured subprocess output (stdout+stderr) to the terminal and log.
    /// Called once per rule after it completes, ensuring atomic non-interleaved output.
    pub fn print_output(&self, output: &[u8]) {
        if output.is_empty() {
            return;
        }
        let s = String::from_utf8_lossy(output);
        // Indent each line so it's visually grouped under the rule.
        for line in s.lines() {
            println!("    {line}");
        }
        // Ensure a trailing newline if the output didn't end with one.
        if !s.ends_with('\n') {
            println!();
        }
        self.log(&s);
    }

    /// Timing summary — printed after a build when more than one rule ran.
    pub fn print_timing_summary(&self, timings: &[(String, std::time::Duration)]) {
        let col = timings.iter().map(|(n, _)| n.len()).max().unwrap_or(0) + 2;
        println!();
        for (name, elapsed) in timings {
            let secs = elapsed.as_secs_f64();
            let time_str = format!("{secs:.2}s");
            println!("  {name:<col$}{}", self.dim(&time_str));
        }
    }

    /// Env-dirty notice.
    pub fn print_env_dirty(&self) {
        println!("{}", self.yellow("  env vars changed — rebuilding all"));
        self.log("  env vars changed — rebuilding all");
    }
}
