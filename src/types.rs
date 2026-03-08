/// A build target — either a named file/artifact or a phony task name.
///
/// `File` targets participate in dirty-checking (hashed after build).
/// `Task` targets always run unless a dep cascade says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    File(String),
    Task(String),
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::File(p) | Target::Task(p) => write!(f, "{p}"),
        }
    }
}

/// A file to download and optionally extract before running a rule's commands.
#[derive(Debug, Clone)]
pub struct Download {
    /// URL to fetch.
    pub url: String,
    /// Local directory to place the downloaded/extracted files.
    pub dest: String,
    /// Archive format to extract: "tar.gz", "tar.xz", "tar.bz2", "tar", "zip", or "none".
    /// When omitted, pbuild infers from the URL extension.
    pub extract: Option<String>,
    /// Strip this many leading path components when extracting (like tar --strip-components).
    pub strip: u32,
}

/// A build rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The artifact or task this rule produces.
    pub target: Target,
    /// Targets that must be up-to-date before this runs.
    pub deps: Vec<Target>,
    /// Source files read by the command (for dirty-checking).
    pub inputs: Vec<String>,
    /// File written by the command (hashed after success; empty for tasks).
    pub output: String,
    /// Path to a compiler-written depfile (Make format). If set, discovered
    /// inputs are merged into the lock file and used for dirty-checking.
    pub depfile: Option<String>,
    /// One or more commands to run sequentially.
    /// Each inner `Vec<String>` is a single argv.
    pub commands: Vec<Vec<String>>,
    /// If true, each command is joined and run via `sh -c "..."`.
    /// Enables shell features: globs, pipes, redirects, `&&`, etc.
    pub shell: bool,
    /// Working directory for the command, relative to pbuild.toml.
    pub dir: Option<String>,
    /// Run `pbuild [target]` in the given subdirectory.
    /// If that directory has no pbuild.toml, falls back to `make [target]`.
    pub subdir: Option<String>,
    /// Run `make [target]` in the given subdirectory.
    pub makedir: Option<String>,
    /// Short description shown in `--list` output.
    pub description: Option<String>,
    /// Group heading for `--list` output (e.g. "Build", "Quality").
    pub group: Option<String>,
    /// Environment variables set for this rule's commands only.
    pub env: std::collections::HashMap<String, String>,
    /// If true, connect stdin to the terminal (for interactive programs like QEMU).
    /// Implies streaming output. Only valid when the rule runs alone (wave size 1).
    pub tty: bool,
    /// If false, skip dirty-checking and always run this rule.
    /// Default is true. Set `cache = false` in pbuild.toml to always re-run.
    pub cache: bool,
    /// If set, run the rule's commands once per file matching this glob.
    /// `{{file}}` in commands is substituted with each matched path.
    pub for_each: Option<String>,
    /// Files to download (and optionally extract) before running commands.
    pub downloads: Vec<Download>,
}
