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

    /// `  ✗ build`
    pub fn print_fail(&self, target: &impl std::fmt::Display) {
        println!("  {} {target}", self.red("✗"));
        self.log(&format!("  ✗ {target}"));
    }

    /// Env-dirty notice.
    pub fn print_env_dirty(&self) {
        println!("{}", self.yellow("  env vars changed — rebuilding all"));
        self.log("  env vars changed — rebuilding all");
    }
}
