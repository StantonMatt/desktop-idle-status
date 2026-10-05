//! XDG paths never resolve against the process working directory.
use std::{ffi::OsString, path::PathBuf};

pub struct Paths<F> {
    get: F,
}
impl<F: FnMut(&str) -> Option<OsString>> Paths<F> {
    pub fn new(get: F) -> Self {
        Self { get }
    }
    pub fn absolute(&mut self, key: &str) -> Option<PathBuf> {
        (self.get)(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    }
    pub fn home(&mut self, key: &str, suffix: &str) -> Option<PathBuf> {
        self.absolute(key)
            .or_else(|| self.absolute("HOME").map(|p| p.join(suffix)))
    }
    pub fn dirs(&mut self, key: &str, default: &str) -> Vec<PathBuf> {
        let value = (self.get)(key)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| default.into());
        std::env::split_paths(&value)
            .filter(|p| p.is_absolute())
            .collect()
    }
}
pub fn environment() -> Paths<impl FnMut(&str) -> Option<OsString>> {
    Paths::new(|key| std::env::var_os(key))
}
pub fn test_root() -> PathBuf {
    let mut paths = environment();
    paths
        .absolute("DESKTOP_IDLE_STATUS_TEST_ROOT")
        .or_else(|| {
            paths
                .absolute("HOME")
                .map(|p| p.join(".cache/agent-scratch/desktop-idle-status/service-tests"))
        })
        .expect("Tests require an absolute DESKTOP_IDLE_STATUS_TEST_ROOT or HOME")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn paths(values: &[(&str, &str)]) -> Paths<impl FnMut(&str) -> Option<OsString>> {
        let values: std::collections::HashMap<_, _> = values
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        Paths::new(move |key| values.get(key).cloned())
    }
    #[test]
    fn home_paths_ignore_empty_and_relative_values() {
        for (key, suffix) in [
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_CACHE_HOME", ".cache"),
            ("XDG_STATE_HOME", ".local/state"),
        ] {
            for invalid in [None, Some(""), Some("relative")] {
                let mut values = vec![("HOME", "/home/test")];
                if let Some(value) = invalid {
                    values.push((key, value));
                }
                assert_eq!(
                    paths(&values).home(key, suffix),
                    Some(PathBuf::from("/home/test").join(suffix))
                );
            }
            assert_eq!(
                paths(&[(key, "/custom")]).home(key, suffix),
                Some("/custom".into())
            );
        }
    }
    #[test]
    fn missing_empty_and_relative_home_never_use_cwd_or_root() {
        for home in [None, Some(""), Some("relative")] {
            let values: Vec<_> = home.map(|v| vec![("HOME", v)]).unwrap_or_default();
            for (key, suffix) in [
                ("XDG_CONFIG_HOME", ".config"),
                ("XDG_DATA_HOME", ".local/share"),
            ] {
                assert_eq!(paths(&values).home(key, suffix), None);
                let mut valid = values.clone();
                valid.push((key, "/custom"));
                assert_eq!(paths(&valid).home(key, suffix), Some("/custom".into()));
            }
        }
    }
    #[test]
    fn search_lists_default_only_when_unset_or_empty_and_filter_invalid_entries() {
        for (key, default) in [
            ("XDG_CONFIG_DIRS", "/etc/xdg"),
            ("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
        ] {
            let expected: Vec<_> = std::env::split_paths(default).collect();
            assert_eq!(paths(&[]).dirs(key, default), expected);
            assert_eq!(paths(&[(key, "")]).dirs(key, default), expected);
            assert_eq!(
                paths(&[(key, ":relative:/one::/two:")]).dirs(key, default),
                vec![PathBuf::from("/one"), PathBuf::from("/two")]
            );
            assert!(paths(&[(key, "relative::")]).dirs(key, default).is_empty());
        }
    }
    #[test]
    fn runtime_has_no_home_or_cwd_fallback() {
        for invalid in [None, Some(""), Some("relative")] {
            let mut values = vec![("HOME", "/home/test")];
            if let Some(value) = invalid {
                values.push(("XDG_RUNTIME_DIR", value));
            }
            assert_eq!(paths(&values).absolute("XDG_RUNTIME_DIR"), None);
        }
        assert_eq!(
            paths(&[("XDG_RUNTIME_DIR", "/run/user/1000")]).absolute("XDG_RUNTIME_DIR"),
            Some("/run/user/1000".into())
        );
    }
    #[test]
    fn paths_preserve_non_utf8_absolute_values() {
        use std::os::unix::ffi::OsStringExt;
        let value = OsString::from_vec(b"/private/\xff".to_vec());
        assert_eq!(
            Paths::new(|_| Some(value.clone())).home("XDG_DATA_HOME", ".local/share"),
            Some(PathBuf::from(value))
        );
    }
}
