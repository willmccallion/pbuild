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
            Target::File(p) => write!(f, "File({p})"),
            Target::Task(n) => write!(f, "Task({n})"),
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
    /// argv — e.g. `["cc", "-c", "foo.c", "-o", "foo.o"]`
    pub command: Vec<String>,
}
