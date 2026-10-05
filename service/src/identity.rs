use crate::config::Ini;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

fn message_locale_from(mut get: impl FnMut(&str) -> Option<String>) -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| get(key).filter(|value| !value.is_empty()))
        .unwrap_or_else(|| "C".into())
}

pub fn strip_title(title: &str, app: &str) -> String {
    for sep in [" — ", " - "] {
        if let Some(prefix) = title.strip_suffix(&format!("{sep}{app}")) {
            return prefix.into();
        }
    }
    title.into()
}
#[derive(Default)]
pub struct DesktopIndex {
    entries: HashMap<String, (String, String)>,
}
impl DesktopIndex {
    pub fn load() -> Self {
        Self::from_paths(&mut crate::paths::environment())
    }
    fn from_paths(
        paths: &mut crate::paths::Paths<impl FnMut(&str) -> Option<std::ffi::OsString>>,
    ) -> Self {
        Self::from_dirs(
            paths
                .home("XDG_DATA_HOME", ".local/share")
                .into_iter()
                .chain(paths.dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"))
                .collect(),
        )
    }

    pub fn from_dirs(dirs: Vec<PathBuf>) -> Self {
        let locale = message_locale_from(|key| std::env::var(key).ok());
        Self::from_dirs_for_locale(dirs, &locale)
    }
    fn from_dirs_for_locale(dirs: Vec<PathBuf>, locale: &str) -> Self {
        let mut this = Self::default();
        for dir in dirs {
            this.scan(&dir.join("applications"), &dir.join("applications"), locale);
        }
        this
    }
    fn scan(&mut self, base: &Path, dir: &Path, locale: &str) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                self.scan(base, &path, locale);
                continue;
            }
            if path.extension().is_none_or(|v| v != "desktop") {
                continue;
            }
            let ini = Ini::read(&path);
            let Some(name) = ini.localized("Desktop Entry", "Name", locale) else {
                continue;
            };
            let id = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('/', "-")
                .trim_end_matches(".desktop")
                .to_owned();
            let value = (
                name.to_owned(),
                ini.localized("Desktop Entry", "Icon", locale)
                    .unwrap_or("application-x-executable")
                    .to_owned(),
            );
            self.entries
                .entry(id.clone())
                .or_insert_with(|| value.clone());
            self.entries.entry(id.to_lowercase()).or_insert(value);
        }
    }
    pub fn resolve(
        &self,
        desktop: &str,
        app_id: &str,
        class: &str,
        executable: &str,
    ) -> (String, String) {
        for id in [desktop, app_id] {
            let id = id.trim_end_matches(".desktop");
            if let Some(v) = self
                .entries
                .get(id)
                .or_else(|| self.entries.get(&id.to_lowercase()))
            {
                return v.clone();
            }
        }
        let name = [
            class,
            app_id,
            executable.rsplit('/').next().unwrap_or(executable),
        ]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("Unknown application");
        (name.into(), "application-x-executable".into())
    }
    pub fn policy(&self, who: &str) -> (String, String) {
        let name = who.rsplit('/').next().unwrap_or(who);
        self.resolve(name, name, "", who)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn message_locale_obeys_environment_precedence_without_global_mutation() {
        for (all, messages, lang, expected) in [
            (Some("C"), Some("es_CL.UTF-8"), Some("zh_TW"), "C"),
            (None, Some("es_CL.UTF-8"), Some("zh_TW"), "es_CL.UTF-8"),
            (Some(""), Some(""), Some("zh_TW"), "zh_TW"),
            (None, None, None, "C"),
        ] {
            assert_eq!(
                message_locale_from(|key| match key {
                    "LC_ALL" => all,
                    "LC_MESSAGES" => messages,
                    "LANG" => lang,
                    _ => unreachable!(),
                }
                .map(str::to_owned)),
                expected
            );
        }
    }
    #[test]
    fn localized_desktop_index_and_caption_suffixes() {
        let root = crate::history::test_dir();
        let applications = root.path().join("applications/nested");
        std::fs::create_dir_all(&applications).unwrap();
        std::fs::write(applications.join("monitor.desktop"), "[Desktop Entry]\nName=System Monitor\nName[es]=Monitor del sistema\nName[zh_TW]=系統監控\nIcon=monitor\nIcon[es]=monitor-es").unwrap();
        std::fs::write(
            applications.join("base.desktop"),
            "[Desktop Entry]\nName=Base only\nIcon=base",
        )
        .unwrap();
        for (locale, expected, icon) in [
            ("C.UTF-8", "System Monitor", "monitor"),
            ("es_CL.UTF-8", "Monitor del sistema", "monitor-es"),
            ("zh_TW.UTF-8", "系統監控", "monitor"),
            ("fr_FR.UTF-8", "System Monitor", "monitor"),
        ] {
            let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], locale);
            let (name, actual_icon) = index.resolve("nested-monitor", "", "", "");
            assert_eq!(name, expected);
            assert_eq!(actual_icon, icon);
            assert_eq!(strip_title(&format!("CPU — {name}"), &name), "CPU");
            assert_eq!(index.resolve("nested-base", "", "", "").0, "Base only");
        }
    }
    #[test]
    fn strips_only_matching_suffix() {
        assert_eq!(strip_title("YouTube — Firefox", "Firefox"), "YouTube");
        assert_eq!(strip_title("Movie - Haruna", "Haruna"), "Movie");
        assert_eq!(
            strip_title("YouTube — Firefox", "Other"),
            "YouTube — Firefox"
        );
    }
    #[test]
    fn xdg_precedence_and_fallback() {
        let root = crate::history::test_dir();
        let a = root.path().join("user/applications");
        let b = root.path().join("system/applications");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(
            a.join("org.app.desktop"),
            "[Desktop Entry]\nName=User app\nIcon=my-icon",
        )
        .unwrap();
        std::fs::write(
            b.join("org.app.desktop"),
            "[Desktop Entry]\nName=System app",
        )
        .unwrap();
        let index =
            DesktopIndex::from_dirs(vec![root.path().join("user"), root.path().join("system")]);
        assert_eq!(
            index.resolve("org.app", "", "", ""),
            ("User app".into(), "my-icon".into())
        );
        assert_eq!(
            index.resolve("", "", "WindowClass", "/bin/foo").0,
            "WindowClass"
        );
    }
    #[test]
    fn desktop_scanning_uses_home_fallback_and_filters_search_entries() {
        let dir = crate::history::test_dir();
        let home = dir.path().join("home");
        let user = home.join(".local/share/applications");
        let system = dir.path().join("system/applications");
        for path in [&user, &system] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(
            user.join("sample.desktop"),
            "[Desktop Entry]\nName=User\nIcon=user",
        )
        .unwrap();
        std::fs::write(
            system.join("sample.desktop"),
            "[Desktop Entry]\nName=System\nIcon=system",
        )
        .unwrap();
        let search = format!(":relative:{}::", system.parent().unwrap().display());
        for invalid in [None, Some(""), Some("relative")] {
            let mut paths = crate::paths::Paths::new(|key| match key {
                "HOME" => Some(home.as_os_str().to_owned()),
                "XDG_DATA_HOME" => invalid.map(Into::into),
                "XDG_DATA_DIRS" => Some(search.clone().into()),
                _ => None,
            });
            let index = DesktopIndex::from_paths(&mut paths);
            assert_eq!(
                index.resolve("sample", "", "", ""),
                ("User".into(), "user".into())
            );
        }
        std::fs::remove_file(user.join("sample.desktop")).unwrap();
        let mut paths = crate::paths::Paths::new(|key| match key {
            "HOME" => Some(home.as_os_str().to_owned()),
            "XDG_DATA_HOME" => Some("".into()),
            "XDG_DATA_DIRS" => Some(search.clone().into()),
            _ => None,
        });
        assert_eq!(
            DesktopIndex::from_paths(&mut paths).resolve("sample", "", "", ""),
            ("System".into(), "system".into())
        );
    }
}
