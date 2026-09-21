use std::ffi::OsString;
use std::path::PathBuf;

/// The user's home directory: a non-empty `HOME` wins, then the platform lookup.
///
/// On unix this is what `dirs::home_dir` already does. On Windows `dirs` asks the
/// shell API (`FOLDERID_Profile`) and reads no environment variable, so pk used to
/// disagree with itself there: `expand_tilde` read only `HOME` — unset under cmd and
/// PowerShell, which left a literal `~` directory in the cwd — while every other
/// site ignored `HOME` entirely. One resolver, one answer.
pub fn home_dir() -> Option<PathBuf> {
    resolve_home(std::env::var_os("HOME"), dirs::home_dir)
}

fn resolve_home(
    env_home: Option<OsString>,
    platform_home: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match env_home {
        Some(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => platform_home(),
    }
}

/// Expand a leading `~/` against [`home_dir`]. Anything else is returned unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    expand_tilde_with(path, home_dir())
}

fn expand_tilde_with(path: &str, home: Option<PathBuf>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_home_variable_wins_over_the_platform_lookup() {
        let home = resolve_home(Some(OsString::from("/injected")), || {
            Some(PathBuf::from("/platform"))
        });

        assert_eq!(home, Some(PathBuf::from("/injected")));
    }

    #[test]
    fn an_unset_home_variable_falls_back_to_the_platform_lookup() {
        let home = resolve_home(None, || Some(PathBuf::from("/platform")));

        assert_eq!(home, Some(PathBuf::from("/platform")));
    }

    #[test]
    fn an_empty_home_variable_is_treated_as_unset() {
        let home = resolve_home(Some(OsString::new()), || Some(PathBuf::from("/platform")));

        assert_eq!(home, Some(PathBuf::from("/platform")));
    }

    #[test]
    fn a_tilde_path_expands_against_the_home_directory() {
        let expanded = expand_tilde_with("~/.prometheus/knowledge", Some(PathBuf::from("/home/u")));

        assert_eq!(
            expanded,
            PathBuf::from("/home/u").join(".prometheus/knowledge")
        );
        assert!(!expanded.starts_with("~"));
    }

    #[test]
    fn a_path_without_a_tilde_prefix_is_returned_unchanged() {
        let expanded = expand_tilde_with("relative/kb", Some(PathBuf::from("/home/u")));

        assert_eq!(expanded, PathBuf::from("relative/kb"));
    }
}
