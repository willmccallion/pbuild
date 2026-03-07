/// Parse a Make-style depfile into a list of dependency paths.
///
/// Depfiles are written by compilers with `-MF foo.d` and look like:
///
/// ```text
/// foo.o: foo.c include/foo.h \
///   include/util.h
/// ```
///
/// We only care about the paths on the right-hand side of the `:`.
/// Backslash-newline continuations and duplicate paths are handled.
#[must_use]
pub fn parse(src: &str) -> Vec<String> {
    let Some((_, rhs)) = src.split_once(':') else {
        return Vec::new();
    };

    // Join continuation lines, then split on whitespace.
    let joined = rhs.replace("\\\n", " ");

    let mut paths: Vec<String> = joined
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();

    // Deduplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_dep() {
        let paths = parse("foo.o: foo.c\n");
        assert_eq!(paths, ["foo.c"]);
    }

    #[test]
    fn multiple_deps_on_one_line() {
        let paths = parse("foo.o: foo.c include/foo.h include/bar.h\n");
        assert_eq!(paths, ["foo.c", "include/foo.h", "include/bar.h"]);
    }

    #[test]
    fn continuation_lines() {
        let paths = parse("foo.o: foo.c \\\n  include/foo.h \\\n  include/bar.h\n");
        assert_eq!(paths, ["foo.c", "include/foo.h", "include/bar.h"]);
    }

    #[test]
    fn duplicates_removed() {
        let paths = parse("foo.o: foo.c foo.c include/foo.h\n");
        assert_eq!(paths, ["foo.c", "include/foo.h"]);
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn no_colon_returns_empty() {
        assert!(parse("foo.o foo.c").is_empty());
    }
}
