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
    /// Short description shown in `--list` output.
    pub description: Option<String>,
    /// Group heading for `--list` output (e.g. "Build", "Quality").
    pub group: Option<String>,
}
