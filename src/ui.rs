/// Terminal output helpers — colored status lines and build timing.
///
/// Colors are suppressed automatically when stdout is not a TTY
/// (e.g. when piped to a file or CI log capture), unless overridden
/// via `[ui] color` in `pbuild.toml`.
use std::io::IsTerminal as _;

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

    fn green<'a>(&self, s: &'a str)  -> std::borrow::Cow<'a, str> { self.c("\x1b[32m", s) }
    fn yellow<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> { self.c("\x1b[33m", s) }
    fn red<'a>(&self, s: &'a str)    -> std::borrow::Cow<'a, str> { self.c("\x1b[31m", s) }
    fn dim<'a>(&self, s: &'a str)    -> std::borrow::Cow<'a, str> { self.c("\x1b[2m",  s) }
    fn bold<'a>(&self, s: &'a str)   -> std::borrow::Cow<'a, str> { self.c("\x1b[1m",  s) }

    /// `› build`  — printed when a rule starts.
    pub fn print_start(&self, target: &impl std::fmt::Display) {
        println!("{} {target}", self.bold(self.prefix()));
    }

    /// `    $ cargo build --release`  — each command within a rule.
    pub fn print_command(&self, cmd: &[String]) {
        println!("    {} {}", self.dim("$"), self.dim(&cmd.join(" ")));
    }

    /// `– build`  (dim, verbose only)
    pub fn print_skip(&self, target: &impl std::fmt::Display) {
        println!("{} {target}", self.dim("–"));
    }

    /// `    (dry) cargo build --release`
    pub fn print_dry_run(&self, cmd: &[String]) {
        println!("    {} {}", self.yellow("dry"), self.dim(&cmd.join(" ")));
    }

    /// `  ✓ build  0.31s`
    pub fn print_done(&self, target: &impl std::fmt::Display, elapsed: std::time::Duration) {
        let secs = elapsed.as_secs_f64();
        let time_str = format!("{secs:.2}s");
        let time = self.dim(&time_str);
        println!("  {} {target}  {time}", self.green("✓"));
    }

    /// `  ✗ build`
    pub fn print_fail(&self, target: &impl std::fmt::Display) {
        println!("  {} {target}", self.red("✗"));
    }

    /// Env-dirty notice.
    pub fn print_env_dirty(&self) {
        println!("{}", self.yellow("  env vars changed — rebuilding all"));
    }
}
