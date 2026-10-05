//! Read-only INI/KConfig subset. Profile defaults match PowerDevil 6.6.6 (non-mobile).
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
struct Value {
    raw: String,
    text: String,
}
#[derive(Clone, Default, Debug)]
pub struct Ini {
    values: HashMap<(String, String), Option<Value>>,
    immutable_keys: HashSet<(String, String)>,
    immutable_groups: HashSet<String>,
    immutable_file: bool,
}
impl Ini {
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        let mut group = String::new();
        let mut file_immutable = false;
        let mut group_immutable = false;
        for line in text.lines().map(trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                // Only a final, exact [$i] is a group/file option. [$d], [$e]
                // and combined group options are literal subgroup names.
                group_immutable = file_immutable;
                let mut names = Vec::new();
                let mut rest = line;
                let mut valid = true;
                while let Some(part) = rest.strip_prefix('[') {
                    let Some((name, tail)) = part.split_once(']') else {
                        valid = false;
                        break;
                    };
                    if name == "$i" && tail.is_empty() {
                        if names.is_empty() {
                            file_immutable = true;
                        } else {
                            group_immutable = true;
                        }
                    } else {
                        names.push(unescape(name));
                    }
                    rest = tail;
                }
                if !valid {
                    continue;
                }
                group = names.join("\u{1d}");
                if group_immutable || file_immutable {
                    out.immutable_groups.insert(group.clone());
                }
                continue;
            }
            let (mut key, value) = line
                .split_once('=')
                .map_or((line, None), |(k, v)| (trim(k), Some(trim(v))));
            if key.is_empty() {
                continue;
            }
            let mut immutable = group_immutable;
            let mut expansion = false;
            let mut deleted = false;
            let mut locale = None;
            let mut valid = true;
            // KConfig scans key qualifiers from right to left; deletion ends
            // parsing immediately, including options later in the same token.
            'options: while let Some(start) = key.rfind('[') {
                let Some(end) = key[start..].find(']').map(|end| start + end) else {
                    valid = false;
                    break;
                };
                let option = &key[start + 1..end];
                if let Some(flags) = option.strip_prefix('$') {
                    for flag in flags.chars() {
                        match flag {
                            'i' => immutable = true,
                            'e' => expansion = true,
                            'd' => {
                                key = &key[..start];
                                deleted = true;
                                break 'options;
                            }
                            _ => {}
                        }
                    }
                } else if locale.replace(option).is_some() {
                    valid = false;
                    break;
                }
                key = &key[..start];
            }
            if !valid || (!deleted && value.is_none()) {
                continue;
            }
            let mut key = unescape(key);
            if !deleted && let Some(locale) = locale {
                key.push('[');
                key.push_str(locale);
                key.push(']');
            }
            let id = (group.clone(), key);
            if out.immutable_keys.contains(&id) {
                continue;
            }
            if immutable {
                out.immutable_keys.insert(id.clone());
            }
            let value = if deleted {
                None
            } else {
                let raw = unescape(value.unwrap());
                let text = if expansion { expand(&raw) } else { raw.clone() };
                Some(Value { raw, text })
            };
            out.values.insert(id, value);
        }
        out.immutable_file = file_immutable;
        out
    }
    pub fn read(path: &Path) -> Self {
        match Self::try_read(path) {
            Ok(ini) => ini,
            Err(e) => {
                eprintln!("Cannot read {}: {e}", path.display());
                Self::default()
            }
        }
    }
    pub fn try_read(path: &Path) -> std::io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }
    /// Merge low to high priority; immutable entries/groups lock subsequent layers.
    pub fn merge(&mut self, layer: Self) {
        if self.immutable_file {
            return;
        }
        for (id, value) in layer.values {
            if !self.immutable_keys.contains(&id) && !self.group_locked(&id.0) {
                self.values.insert(id.clone(), value);
                if layer.immutable_keys.contains(&id) {
                    self.immutable_keys.insert(id);
                }
            }
        }
        self.immutable_groups.extend(layer.immutable_groups);
        self.immutable_file = layer.immutable_file;
    }
    fn group_locked(&self, group: &str) -> bool {
        self.immutable_file || self.immutable_groups.contains(group)
    }
    pub fn cascade(dirs: &[PathBuf], name: &str) -> Self {
        let mut ini = Self::default();
        for dir in dirs {
            ini.merge(Self::read(&dir.join(name)));
        }
        ini
    }
    pub fn get(&self, group: &str, key: &str) -> Option<&str> {
        self.values
            .get(&(group.replace('/', "\u{1d}"), key.into()))
            .and_then(|v| v.as_ref().map(|v| v.text.as_str()))
    }
    /// Desktop Entry locale matching: territory precedes modifier, then language,
    /// then the unqualified key. Encoding does not participate in matching.
    pub fn localized(&self, group: &str, key: &str, locale: &str) -> Option<&str> {
        let (locale, modifier) = locale
            .split_once('@')
            .map_or((locale, None), |(locale, modifier)| {
                (locale, Some(modifier))
            });
        let locale = locale.split('.').next().unwrap_or(locale);
        if !matches!(locale, "" | "C" | "POSIX") {
            let (language, country) = locale
                .split_once('_')
                .map_or((locale, None), |(language, country)| {
                    (language, Some(country))
                });
            let mut candidates = Vec::new();
            if let Some(country) = country {
                if let Some(modifier) = modifier {
                    candidates.push(format!("{language}_{country}@{modifier}"));
                }
                candidates.push(format!("{language}_{country}"));
            }
            if let Some(modifier) = modifier {
                candidates.push(format!("{language}@{modifier}"));
            }
            candidates.push(language.to_owned());
            for locale in candidates {
                if let Some(value) = self.get(group, &format!("{key}[{locale}]")) {
                    return Some(value);
                }
            }
        }
        self.get(group, key)
    }
    // KConfigGroup's numeric/bool QVariant reads do not expand [$e]. Only
    // QString reads (desktop identity strings here) perform expansion.
    fn raw(&self, group: &str, key: &str) -> Option<&str> {
        self.values
            .get(&(group.replace('/', "\u{1d}"), key.into()))
            .and_then(|v| v.as_ref().map(|v| v.raw.as_str()))
    }
    pub fn number(&self, group: &str, key: &str, default: f64) -> f64 {
        self.raw(group, key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    }
    pub fn integer(&self, group: &str, key: &str, default: i32) -> i32 {
        self.raw(group, key)
            .and_then(|v| trim(v).parse::<i64>().ok())
            .map(|v| v as i32) // QVariant converts QByteArray via qlonglong, then narrows.
            .unwrap_or(default)
    }
    pub fn unsigned(&self, group: &str, key: &str, default: u32) -> u32 {
        self.raw(group, key)
            .and_then(|v| trim(v).parse::<u64>().ok())
            .map(|v| v as u32)
            .unwrap_or(default)
    }
    pub fn boolean(&self, group: &str, key: &str, default: bool) -> bool {
        self.raw(group, key).map_or(default, |v| {
            !["false", "no", "off", "0"]
                .iter()
                .any(|n| v.eq_ignore_ascii_case(n))
        })
    }
}
// QByteArrayView::trimmed uses ASCII whitespace, not Unicode whitespace.
fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c == ' ' || ('\t'..='\r').contains(&c))
}
fn unescape(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_owned();
    }
    // Mirror printableToString's in-place decoding, including its partial
    // original buffer on an invalid escape (KF 6.24).
    let mut bytes = trim(text).as_bytes().to_vec();
    let len = bytes.len();
    let (mut i, mut written) = (0, 0);
    while i < len {
        if bytes[i] != b'\\' || i + 1 == len {
            bytes[written] = bytes[i];
        } else {
            i += 1;
            bytes[written] = match bytes[i] {
                b's' => b' ',
                b't' => b'\t',
                b'n' => b'\n',
                b'r' => b'\r',
                b'\\' => b'\\',
                b';' | b',' => {
                    bytes[written] = b'\\';
                    written += 1;
                    bytes[i]
                }
                b'x' => {
                    let value = bytes.get(i + 1..i + 3).and_then(|hex| {
                        let a = (hex[0] as char).to_digit(16)?;
                        let b = (hex[1] as char).to_digit(16)?;
                        Some((a * 16 + b) as u8)
                    });
                    i = (i + 2).min(len - 1);
                    value.unwrap_or(b'x')
                }
                _ => {
                    bytes[written] = b'\\';
                    return String::from_utf8_lossy(&bytes).into_owned();
                }
            };
        }
        written += 1;
        i += 1;
    }
    bytes.truncate(written);
    String::from_utf8_lossy(&bytes).into_owned()
}
fn expand(text: &str) -> String {
    expand_from(text, |key| std::env::var_os(key))
}
fn expand_from(text: &str, mut get: impl FnMut(&str) -> Option<std::ffi::OsString>) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' || chars.peek().is_none() {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'$') {
            chars.next();
            out.push('$');
            continue;
        }
        let mut name = String::new();
        if chars.peek() == Some(&'{') {
            chars.next();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
        } else {
            while chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                name.push(chars.next().unwrap());
            }
        }
        if !name.is_empty() {
            let mut paths = crate::paths::Paths::new(&mut get);
            if let Some(default) = match name.as_str() {
                "XDG_CONFIG_DIRS" => Some("/etc/xdg"),
                "XDG_DATA_DIRS" => Some("/usr/local/share:/usr/share"),
                _ => None,
            } {
                let dirs = paths.dirs(&name, default);
                out.push_str(
                    &std::env::join_paths(dirs)
                        .unwrap_or_default()
                        .to_string_lossy(),
                );
                continue;
            }

            let path = match name.as_str() {
                "HOME" | "XDG_RUNTIME_DIR" => Some(paths.absolute(&name)),
                "XDG_DATA_HOME" => Some(paths.home(&name, ".local/share")),
                "XDG_CONFIG_HOME" => Some(paths.home(&name, ".config")),
                "XDG_CACHE_HOME" => Some(paths.home(&name, ".cache")),
                "XDG_STATE_HOME" => Some(paths.home(&name, ".local/state")),
                "QT_DATA_HOME" => Some(
                    paths
                        .absolute(&name)
                        .or_else(|| paths.home("XDG_DATA_HOME", ".local/share")),
                ),
                "QT_CONFIG_HOME" => Some(
                    paths
                        .absolute(&name)
                        .or_else(|| paths.home("XDG_CONFIG_HOME", ".config")),
                ),
                "QT_CACHE_HOME" => Some(
                    paths
                        .absolute(&name)
                        .or_else(|| paths.home("XDG_CACHE_HOME", ".cache")),
                ),
                _ => None,
            };
            let value = match path {
                Some(value) => value
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                None => get(&name)
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            };
            out.push_str(&value);
        }
    }
    out
}
/// XDG lists are highest priority first. KConfig merges system layers in reverse,
/// then the user layer, retaining immutable values from lower priority layers.
pub fn source_dirs(user: &Path) -> Vec<PathBuf> {
    let mut dirs = crate::paths::environment().dirs("XDG_CONFIG_DIRS", "/etc/xdg");
    dirs.reverse();
    dirs.push(user.to_owned());
    dirs
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub setting: String,
    pub seconds: u32,
    pub kcm: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Ac,
    Battery,
    LowBattery,
}
impl Profile {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "AC" => Some(Self::Ac),
            "Battery" => Some(Self::Battery),
            "LowBattery" => Some(Self::LowBattery),
            _ => None,
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Ac => "AC",
            Self::Battery => "Battery",
            Self::LowBattery => "LowBattery",
        }
    }
    fn defaults(self) -> (i32, i32, i32) {
        match self {
            Self::Ac => (300, 600, 900),
            Self::Battery => (120, 300, 600),
            Self::LowBattery => (60, 120, 300),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub timeout: u32,
    pub conflicts: Vec<Conflict>,
}
// PowerDevil v6.6.6 daemon/powerdevilenums.h and
// actions/bundled/suspendsession.cpp::triggerImpl. These are discrete enum
// values, not flags. 4 is retired hybrid sleep; SleepMode selects sleep variants.
fn auto_action_conflict(action: u32) -> Option<&'static str> {
    match action {
        1 | 2 | 8 => Some("suspend"), // Sleep, Hibernate, Shutdown
        32 => Some("lock"),
        // NoAction=0; PromptLogoutDialog=16 doesn't force logout. Screen actions
        // 64/128 are handled by power-button/lid code, not the auto-idle action.
        _ => None,
    }
}
/// Retain each valid layer on I/O failures, retrying even without a watcher event.
#[derive(Default)]
pub struct ConfigReader {
    layers: HashMap<PathBuf, Ini>,
    legacy_global: Option<PathBuf>,
    pub retry_pending: bool,
}
impl ConfigReader {
    /// The daemon includes KConfig's legacy global file; tests supply a scratch
    /// path or omit it. KDE_SKIP_KDERC disables it just as in KConfig.
    pub fn desktop() -> Self {
        Self {
            legacy_global: std::env::var_os("KDE_SKIP_KDERC")
                .is_none()
                .then(|| PathBuf::from("/etc/kde5rc")),
            ..Self::default()
        }
    }
    pub fn legacy_global(&self) -> Option<&Path> {
        self.legacy_global.as_deref()
    }
    fn layer(&mut self, path: PathBuf) -> Ini {
        match Ini::try_read(&path) {
            Ok(layer) => {
                self.layers.insert(path.clone(), layer);
            }
            Err(e) => {
                self.retry_pending = true;
                eprintln!(
                    "Cannot read {}; retaining last valid layer: {e}",
                    path.display()
                );
            }
        }
        self.layers.get(&path).cloned().unwrap_or_default()
    }
    fn cascade(&mut self, dirs: &[PathBuf], name: &str) -> Ini {
        let mut ini = Ini::default();
        for dir in dirs {
            ini.merge(self.layer(dir.join(name)));
        }
        ini
    }
    pub fn read(
        &mut self,
        dirs: &[PathBuf],
        can_suspend: bool,
        is_vm: bool,
        profile: Option<Profile>,
    ) -> Config {
        self.retry_pending = false;
        let mut globals = self
            .legacy_global
            .clone()
            .map_or_else(Ini::default, |path| self.layer(path));
        globals.merge(self.cascade(dirs, "system.kdeglobals"));
        globals.merge(self.cascade(dirs, "kdeglobals"));
        let mut saver = globals.clone();
        saver.merge(self.cascade(dirs, "plasma-visual-screensaverrc"));
        let mut power = globals.clone();
        power.merge(self.cascade(dirs, "powerdevilrc"));
        let mut lock = globals;
        lock.merge(self.cascade(dirs, "kscreenlockerrc"));
        Config::parse(&saver, &power, &lock, can_suspend, is_vm, profile)
    }
}
impl Config {
    pub fn read(dir: &Path, can_suspend: bool, is_vm: bool, profile: Option<Profile>) -> Self {
        Self::read_sources(&source_dirs(dir), can_suspend, is_vm, profile)
    }
    pub fn read_sources(
        dirs: &[PathBuf],
        can_suspend: bool,
        is_vm: bool,
        profile: Option<Profile>,
    ) -> Self {
        ConfigReader::default().read(dirs, can_suspend, is_vm, profile)
    }
    pub fn parse(
        saver: &Ini,
        power: &Ini,
        lock: &Ini,
        can_suspend: bool,
        is_vm: bool,
        profile: Option<Profile>,
    ) -> Self {
        let timeout = saver.integer("General", "IdleMinutes", 10).clamp(1, 240) as u32 * 60;
        let mut conflicts: Vec<Conflict> = Vec::new();
        let mut deadlines: HashMap<String, f64> = HashMap::new();
        let mut add = |setting: &str, enabled: bool, seconds: f64, kcm: &str| {
            if enabled && seconds >= 0.0 && seconds <= f64::from(timeout) {
                // Locking can come from three independent settings. Report the
                // earliest deadline and the KCM responsible for that deadline.
                if let Some(existing) = conflicts.iter_mut().find(|c| c.setting == setting) {
                    if seconds < deadlines[setting] {
                        deadlines.insert(setting.into(), seconds);
                        existing.seconds = seconds as u32;
                        existing.kcm = kcm.into();
                    }
                    return;
                }
                deadlines.insert(setting.into(), seconds);
                conflicts.push(Conflict {
                    setting: setting.into(),
                    seconds: seconds as u32,
                    kcm: kcm.into(),
                });
            }
        };
        // Unknown active profile must not be reported as AC. Locking is independent.
        let (display, suspend, defaults) = profile.map_or_else(
            || (String::new(), String::new(), (0, 0, 0)),
            |p| {
                (
                    format!("{}/Display", p.id()),
                    format!("{}/SuspendAndShutdown", p.id()),
                    p.defaults(),
                )
            },
        );
        add(
            "dim",
            profile.is_some() && power.boolean(&display, "DimDisplayWhenIdle", true),
            f64::from(power.integer(&display, "DimDisplayIdleTimeoutSec", defaults.0)),
            "kcm_powerdevilprofilesconfig",
        );
        let screen_off =
            profile.is_some() && power.boolean(&display, "TurnOffDisplayWhenIdle", true);
        let off_time =
            f64::from(power.integer(&display, "TurnOffDisplayIdleTimeoutSec", defaults.1));
        add(
            "screen-off",
            screen_off,
            off_time,
            "kcm_powerdevilprofilesconfig",
        );
        let lock_time = lock.number("Daemon", "Timeout", 5.0) * 60.0;
        let lock_before_off = screen_off
            && power.boolean(&display, "LockBeforeTurnOffDisplay", false)
            && off_time >= 0.0;
        let auto_lock = lock.boolean("Daemon", "Autolock", true) && lock_time > 0.0;
        add("lock", auto_lock, lock_time, "kcm_screenlocker");
        add(
            "lock",
            lock_before_off,
            off_time,
            "kcm_powerdevilprofilesconfig",
        );
        let action = power.unsigned(
            &suspend,
            "AutoSuspendAction",
            if can_suspend && !is_vm { 1 } else { 0 },
        );
        let suspend_time =
            f64::from(power.integer(&suspend, "AutoSuspendIdleTimeoutSec", defaults.2));
        // SuspendSession::loadAction does not register a zero idle timeout.
        if let Some(setting) = auto_action_conflict(action) {
            add(
                setting,
                profile.is_some() && suspend_time > 0.0,
                suspend_time,
                "kcm_powerdevilprofilesconfig",
            );
        }
        conflicts.sort_by_key(|c| {
            (
                c.seconds,
                match c.setting.as_str() {
                    "lock" => 0,
                    "screen-off" => 1,
                    "suspend" => 2,
                    _ => 3,
                },
            )
        });
        Self { timeout, conflicts }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_config_globals_override_delete_and_immutability() {
        let root = crate::history::test_dir();
        let dirs = vec![root.path().into()];
        let global = root.path().join("kdeglobals");
        std::fs::write(
            root.path().join("system.kdeglobals"),
            "[General]\nIdleMinutes=3",
        )
        .unwrap();
        assert_eq!(
            Config::read_sources(&dirs, true, false, Some(Profile::Ac)).timeout,
            180
        );
        let legacy = root.path().join("kde5rc");
        std::fs::write(&legacy, "[General]\nIdleMinutes=4").unwrap();
        let mut legacy_reader = ConfigReader {
            legacy_global: Some(legacy),
            ..Default::default()
        };
        assert_eq!(
            legacy_reader
                .read(&dirs, true, false, Some(Profile::Ac))
                .timeout,
            180
        );
        std::fs::remove_file(root.path().join("system.kdeglobals")).unwrap();
        assert_eq!(
            legacy_reader
                .read(&dirs, true, false, Some(Profile::Ac))
                .timeout,
            240
        );
        std::fs::write(&global, "[General]\nIdleMinutes=1\n[AC][Display]\nDimDisplayIdleTimeoutSec=15\n[Daemon]\nTimeout=0.5").unwrap();
        let read = || Config::read_sources(&dirs, true, false, Some(Profile::Ac));
        let c = read();
        assert_eq!(c.timeout, 60);
        assert_eq!(
            c.conflicts.iter().map(|c| c.seconds).collect::<Vec<_>>(),
            [15, 30]
        );
        let app = root.path().join("plasma-visual-screensaverrc");
        std::fs::write(&app, "[General]\nIdleMinutes=2").unwrap();
        assert_eq!(read().timeout, 120);
        std::fs::write(&app, "[General]\nIdleMinutes[$d]=").unwrap();
        assert_eq!(read().timeout, 600);
        std::fs::write(&global, "[General]\nIdleMinutes[$i]=1").unwrap();
        std::fs::write(&app, "[General]\nIdleMinutes=2").unwrap();
        assert_eq!(read().timeout, 60);
    }
    #[test]
    fn typed_integer_conversion_matches_qvariant_before_clamping() {
        // Verified with installed KConfig/Qt: integer variants narrow a parsed
        // 64-bit integer, including values outside the 32-bit range.
        for (raw, expected) in [
            ("2.5", 600),
            ("1e2", 600),
            ("bad", 600),
            ("", 600),
            ("9223372036854775808", 600),
            ("2147483648", 60),
            ("4294967297", 60),
            ("-2147483649", 14400),
            ("+002", 120),
            ("0x10", 600),
            ("-1", 60),
        ] {
            let c = Config::parse(
                &Ini::parse(&format!("[General]\nIdleMinutes={raw}")),
                &Ini::default(),
                &Ini::default(),
                true,
                false,
                None,
            );
            assert_eq!(c.timeout, expected, "{raw}");
        }
        for key in [
            "DimDisplayIdleTimeoutSec",
            "TurnOffDisplayIdleTimeoutSec",
            "AutoSuspendIdleTimeoutSec",
        ] {
            for (raw, expected) in [
                ("2.5", 99),
                ("1e2", 99),
                ("2147483648", i32::MIN),
                ("4294967297", 1),
                ("9223372036854775808", 99),
            ] {
                let ini = Ini::parse(&format!("[AC]\n{key}={raw}"));
                assert_eq!(ini.integer("AC", key, 99), expected, "{key}: {raw}");
            }
        }
        let ini = Ini::parse("[AC]\nAction=4294967297\nNegative=-1\nOverflow=18446744073709551616");
        assert_eq!(ini.unsigned("AC", "Action", 0), 1);
        assert_eq!(ini.unsigned("AC", "Negative", 2), 2);
        assert_eq!(ini.unsigned("AC", "Overflow", 2), 2);
    }
    #[test]
    fn lock_conflict_keeps_controlling_page_for_earliest_deadline() {
        for (autolock, lock_minutes, off, expected) in [
            (false, 5.0, 20, "kcm_powerdevilprofilesconfig"),
            (true, 0.5, 20, "kcm_powerdevilprofilesconfig"),
            (true, 0.25, 20, "kcm_screenlocker"),
            (true, 0.5, 30, "kcm_screenlocker"),
            (true, 20.5 / 60.0, 20, "kcm_powerdevilprofilesconfig"),
        ] {
            let c = Config::parse(
                &Ini::default(),
                &Ini::parse(&format!(
                    "[AC][Display]\nLockBeforeTurnOffDisplay=true\nTurnOffDisplayIdleTimeoutSec={off}"
                )),
                &Ini::parse(&format!(
                    "[Daemon]\nAutolock={autolock}\nTimeout={lock_minutes}"
                )),
                true,
                false,
                Some(Profile::Ac),
            );
            assert_eq!(
                c.conflicts
                    .iter()
                    .find(|c| c.setting == "lock")
                    .unwrap()
                    .kcm,
                expected
            );
        }
    }
    #[test]
    fn failed_layers_retain_values_retry_and_missing_layers_are_removed() {
        for name in [
            "system.kdeglobals",
            "kdeglobals",
            "plasma-visual-screensaverrc",
            "powerdevilrc",
            "kscreenlockerrc",
        ] {
            let root = crate::history::test_dir();
            let file = root.path().join(name);
            let text = "[General]\nIdleMinutes=1\n[AC][Display]\nDimDisplayIdleTimeoutSec=15\n[Daemon]\nTimeout=0.5";
            std::fs::write(&file, text).unwrap();
            let dirs = vec![root.path().into()];
            let mut reader = ConfigReader::default();
            let first = reader.read(&dirs, true, false, Some(Profile::Ac));
            std::fs::remove_file(&file).unwrap();
            // A directory at the filename injects EISDIR even when run as root.
            std::fs::create_dir(&file).unwrap();
            assert_eq!(reader.read(&dirs, true, false, Some(Profile::Ac)), first);
            assert!(reader.retry_pending);
            std::fs::remove_dir(&file).unwrap();
            std::fs::write(
                &file,
                text.replace("=1\n", "=2\n")
                    .replace("=15", "=25")
                    .replace("=0.5", "=0.75"),
            )
            .unwrap();
            assert_ne!(reader.read(&dirs, true, false, Some(Profile::Ac)), first);
            assert!(!reader.retry_pending);
            std::fs::remove_file(&file).unwrap();
            assert_eq!(
                reader.read(&dirs, true, false, Some(Profile::Ac)),
                Config::parse(
                    &Ini::default(),
                    &Ini::default(),
                    &Ini::default(),
                    true,
                    false,
                    Some(Profile::Ac)
                )
            );
        }
    }
    #[test]
    fn powerdevil_auto_actions_match_666() {
        // Full PowerButtonAction enum, retired value, unknown and combined flags.
        for (action, expected) in [
            (0, None),
            (1, Some("suspend")),
            (2, Some("suspend")),
            (4, None),
            (8, Some("suspend")),
            (16, None),
            (32, Some("lock")),
            (64, None),
            (128, None),
            (3, None),
            (256, None),
        ] {
            for profile in [Profile::Ac, Profile::Battery, Profile::LowBattery] {
                let power = Ini::parse(&format!(
                    "[{}][Display]\nDimDisplayWhenIdle=false\nTurnOffDisplayWhenIdle=false\n\
                     [{}][SuspendAndShutdown]\nAutoSuspendAction={action}\nAutoSuspendIdleTimeoutSec=30",
                    profile.id(),
                    profile.id()
                ));
                let c = Config::parse(
                    &Ini::default(),
                    &power,
                    &Ini::parse("[Daemon]\nAutolock=false"),
                    true,
                    false,
                    Some(profile),
                );
                assert_eq!(
                    c.conflicts.first().map(|c| c.setting.as_str()),
                    expected,
                    "{profile:?} action {action}"
                );
                assert_eq!(c.conflicts.len(), usize::from(expected.is_some()));
                for conflict in c.conflicts {
                    assert_eq!(conflict.seconds, 30);
                    assert_eq!(conflict.kcm, "kcm_powerdevilprofilesconfig");
                }
            }
        }
    }
    #[test]
    fn auto_action_defaults_and_sleep_modes() {
        for profile in [Profile::Ac, Profile::Battery, Profile::LowBattery] {
            for can_suspend in [false, true] {
                for is_vm in [false, true] {
                    for action in [
                        "",
                        "AutoSuspendAction=invalid",
                        "AutoSuspendAction=1.5",
                        "AutoSuspendAction=-1",
                    ] {
                        let power = Ini::parse(&format!(
                            "[{}][SuspendAndShutdown]\n{action}",
                            profile.id()
                        ));
                        let c = Config::parse(
                            &Ini::parse("[General]\nIdleMinutes=240"),
                            &power,
                            &Ini::default(),
                            can_suspend,
                            is_vm,
                            Some(profile),
                        );
                        assert_eq!(
                            c.conflicts.iter().any(|c| c.setting == "suspend"),
                            can_suspend && !is_vm
                        );
                    }
                }
            }
            for sleep_mode in [1, 2, 3] {
                let power = Ini::parse(&format!(
                    "[{}][SuspendAndShutdown]\nAutoSuspendAction=1\nSleepMode={sleep_mode}",
                    profile.id()
                ));
                let c = Config::parse(
                    &Ini::parse("[General]\nIdleMinutes=240"),
                    &power,
                    &Ini::default(),
                    false,
                    true,
                    Some(profile),
                );
                assert!(c.conflicts.iter().any(|c| c.setting == "suspend"));
            }
        }
    }
    #[test]
    fn auto_action_zero_and_negative_timeouts_are_disabled() {
        for action in [1, 2, 8, 32] {
            for timeout in [0, -1] {
                let power = Ini::parse(&format!(
                    "[AC][Display]\nDimDisplayWhenIdle=false\nTurnOffDisplayWhenIdle=false\n\
                    [AC][SuspendAndShutdown]\nAutoSuspendAction={action}\nAutoSuspendIdleTimeoutSec={timeout}"
                ));
                let c = Config::parse(
                    &Ini::default(),
                    &power,
                    &Ini::parse("[Daemon]\nAutolock=false"),
                    true,
                    false,
                    Some(Profile::Ac),
                );
                assert!(c.conflicts.is_empty());
            }
        }
    }
    #[test]
    fn lock_sources_choose_earliest_deadline_and_kcm() {
        for (auto, screen, expected, kcm) in [
            (20, 40, 20, "kcm_powerdevilprofilesconfig"),
            (50, 40, 30, "kcm_screenlocker"),
        ] {
            let power = Ini::parse(&format!(
                "[AC][Display]\nDimDisplayWhenIdle=false\nTurnOffDisplayIdleTimeoutSec={screen}\nLockBeforeTurnOffDisplay=true\n\
                [AC][SuspendAndShutdown]\nAutoSuspendAction=32\nAutoSuspendIdleTimeoutSec={auto}"
            ));
            let c = Config::parse(
                &Ini::default(),
                &power,
                &Ini::parse("[Daemon]\nTimeout=0.5"),
                true,
                false,
                Some(Profile::Ac),
            );
            let locks: Vec<_> = c.conflicts.iter().filter(|c| c.setting == "lock").collect();
            assert_eq!(locks.len(), 1);
            assert_eq!(locks[0].seconds, expected);
            assert_eq!(locks[0].kcm, kcm);
        }
    }
    #[test]
    fn deletion_entries_follow_kconfig_option_order() {
        for (entry, expected, locked) in [
            ("IdleMinutes[$d]", None, false),
            ("IdleMinutes[$d]=ignored", None, false),
            ("IdleMinutes[$id]", None, true),
            ("IdleMinutes[$di]", None, false),
            ("IdleMinutes[$d][$i]", None, true),
            ("IdleMinutes[$i][$d]", Some("2"), false),
            ("IdleMinutes[$e][$d]", Some("2"), false),
            ("IdleMinutes", Some("2"), false),
            ("IdleMinutes[$i]", Some("2"), false),
            ("IdleMinutes=", Some(""), false),
            ("IdleMinutes[$unknown]=3", Some("3"), false),
            ("IdleMinutes[missing=3", Some("2"), false),
            ("IdleMinutes[a][b]=3", Some("2"), false),
        ] {
            let mut ini = Ini::parse("[General]\nIdleMinutes=2");
            ini.merge(Ini::parse(&format!("[General]\n{entry}")));
            assert_eq!(ini.get("General", "IdleMinutes"), expected, "{entry}");
            ini.merge(Ini::parse("[General]\nIdleMinutes=4"));
            assert_eq!(
                ini.get("General", "IdleMinutes"),
                if locked { None } else { Some("4") },
                "{entry}"
            );
        }
    }
    #[test]
    fn bare_deletions_apply_to_every_config_source() {
        let dir = crate::history::test_dir();
        let low = dir.path().join("low");
        let high = dir.path().join("high");
        std::fs::create_dir(&low).unwrap();
        std::fs::create_dir(&high).unwrap();
        for (file, inherited, deleted, group, key) in [
            (
                "plasma-visual-screensaverrc",
                "[General]\nIdleMinutes=1",
                "[General]\nIdleMinutes[$d]",
                "General",
                "IdleMinutes",
            ),
            (
                "powerdevilrc",
                "[AC][Display]\nDimDisplayWhenIdle=false",
                "[AC][Display]\nDimDisplayWhenIdle[$d]",
                "AC/Display",
                "DimDisplayWhenIdle",
            ),
            (
                "kscreenlockerrc",
                "[Daemon]\nTimeout=1",
                "[Daemon]\nTimeout[$d]",
                "Daemon",
                "Timeout",
            ),
        ] {
            std::fs::write(low.join(file), inherited).unwrap();
            std::fs::write(high.join(file), deleted).unwrap();
            let ini = Ini::cascade(&[low.clone(), high.clone()], file);
            assert_eq!(ini.get(group, key), None, "{file}");
        }
        let config = Config::read_sources(&[low, high], true, false, Some(Profile::Ac));
        assert_eq!(config.timeout, 600);
        assert!(
            config
                .conflicts
                .iter()
                .any(|c| c.setting == "dim" && c.seconds == 300)
        );
        assert!(
            config
                .conflicts
                .iter()
                .any(|c| c.setting == "lock" && c.seconds == 300)
        );
    }
    #[test]
    fn group_options_and_file_immutability_match_kconfig() {
        for header in [
            "[General][$d]",
            "[General][$e]",
            "[General][$id]",
            "[General][$i] # tail",
        ] {
            let mut ini = Ini::parse("[General]\nIdleMinutes=2");
            ini.merge(Ini::parse(&format!("{header}\nIdleMinutes=3")));
            assert_eq!(ini.get("General", "IdleMinutes"), Some("2"), "{header}");
        }
        for system in [
            "[$i]\n[General]\nIdleMinutes=2",
            "[General][$i]\nIdleMinutes=2",
            "[General]\nIdleMinutes[$ie]=2",
        ] {
            let mut ini = Ini::parse(system);
            ini.merge(Ini::parse("[General]\nIdleMinutes=3"));
            assert_eq!(ini.get("General", "IdleMinutes"), Some("2"), "{system}");
        }
        let mut ini = Ini::parse("[$i]\n[General]\nIdleMinutes=2");
        ini.merge(Ini::parse("[Other]\nNew=3"));
        assert_eq!(ini.get("Other", "New"), None);
        let ini = Ini::parse("[AC/Display]\nSeconds=99\n[AC][Display]\nSeconds=10");
        assert_eq!(ini.get("AC/Display", "Seconds"), Some("10"));
        let ini = Ini::parse("[AC/Display]\nSeconds=99");
        assert_eq!(ini.get("AC/Display", "Seconds"), None);
        let ini = Ini::parse("[General]\nIdleMinutes[$i]=2\nIdleMinutes=3");
        assert_eq!(ini.get("General", "IdleMinutes"), Some("2"));
        let ini = Ini::parse("[General][$i]\nIdleMinutes=2\nIdleMinutes=3\n[General]\nOther=4");
        assert_eq!(ini.get("General", "IdleMinutes"), Some("2"));
        assert_eq!(ini.get("General", "Other"), Some("4"));
    }
    #[test]
    fn whitespace_and_escapes_apply_to_keys_groups_and_values() {
        let ini = Ini::parse(
            "  [General] # tail\r\n  IdleMinutes  =  2  \r\nIgnored\n# comment\n=empty-key\n[ General ]\nIdleMinutes=3\n[Gen\\x65ral]\nIdle\\x4dinutes=\\s4\\s\n;Key=value",
        );
        assert_eq!(ini.get("General", "IdleMinutes"), Some(" 4 "));
        assert_eq!(ini.number("General", "IdleMinutes", 10.0), 4.0);
        assert_eq!(ini.get(" General ", "IdleMinutes"), Some("3"));
        assert_eq!(ini.get("General", ";Key"), Some("value"));
        assert_eq!(ini.get("General", "Ignored"), None);
        assert_eq!(unescape(r"a\sb\tc\nd\r\\\;\,\x41"), "a b\tc\nd\r\\\\;\\,A");
        assert_eq!(unescape("trailing\\"), "trailing\\");
        assert_eq!(unescape(r"\xG0\x1"), "xx");
        assert_eq!(unescape(r"\q"), r"\q");
        // QByteArray ASCII trimming leaves this group name distinct.
        let ini = Ini::parse("[\u{a0}General\u{a0}]\nIdleMinutes=8");
        assert_eq!(ini.get("General", "IdleMinutes"), None);
    }
    #[test]
    fn expansion_is_for_strings_and_not_numeric_or_bool_reads() {
        let home = std::env::var("HOME").unwrap();
        let ini = Ini::parse(
            "[General]\nName[$ei]=$HOME/${HOME}/$$/$DESKTOP_IDLE_STATUS_UNKNOWN_TEST_VAR/$\nIdleMinutes[$e]=${HOME}\nEnabled[$e]=${HOME}\nPlain=$HOME",
        );
        assert_eq!(
            ini.get("General", "Name"),
            Some(format!("{home}/{home}/$//$").as_str())
        );
        assert_eq!(ini.get("General", "Plain"), Some("$HOME"));
        assert_eq!(ini.number("General", "IdleMinutes", 10.0), 10.0);
        assert!(ini.boolean("General", "Enabled", false));
        for negative in ["false", "FALSE", "No", "OFF", "0"] {
            assert!(!Ini::parse(&format!("[G]\nB={negative}")).boolean("G", "B", true));
        }
        for positive in ["true", "yes", "on", "1", "", "anything"] {
            assert!(Ini::parse(&format!("[G]\nB={positive}")).boolean("G", "B", false));
        }
    }
    #[test]
    fn localized_keys_preserve_base_and_nonlocalized_siblings() {
        let ini = Ini::parse(
            "[Desktop Entry]\nName=Base\nName[es]=Español\nName[zh_TW]=繁體\n\
             GenericName=Monitor\nGenericName[es]=Monitor del sistema\n\
             Icon=base-icon\nIcon[es]=spanish-icon\n\
             StartupWMClass=base-class\nStartupWMClass[es]=other-class\n\
             Exec=base-command\nExec[es]=other-command",
        );
        for (key, base, translated) in [
            ("Name", "Base", "Español"),
            ("GenericName", "Monitor", "Monitor del sistema"),
            ("Icon", "base-icon", "spanish-icon"),
        ] {
            assert_eq!(ini.get("Desktop Entry", key), Some(base));
            assert_eq!(
                ini.localized("Desktop Entry", key, "es_CL.UTF-8"),
                Some(translated)
            );
            assert_eq!(ini.localized("Desktop Entry", key, "fr_FR"), Some(base));
        }
        for (key, base) in [("StartupWMClass", "base-class"), ("Exec", "base-command")] {
            assert_eq!(ini.get("Desktop Entry", key), Some(base));
        }
        assert_eq!(ini.get("Desktop Entry", "Name[zh_TW]"), Some("繁體"));
    }
    #[test]
    fn locale_matching_uses_spec_fallback_order() {
        for (translations, locale, expected) in [
            ("Name[sr_RS@latin]=exact", "sr_RS.UTF-8@latin", "exact"),
            (
                "Name[sr_RS]=country\nName[sr@latin]=modifier",
                "sr_RS@latin",
                "country",
            ),
            (
                "Name[sr@latin]=modifier\nName[sr]=language",
                "sr_RS@latin",
                "modifier",
            ),
            ("Name[sr]=language", "sr_RS@latin", "language"),
            ("Name[sr@latin]=modifier", "sr@latin", "modifier"),
            ("Name[sr]=language", "sr", "language"),
            ("Name[zh_TW]=繁體\nName[zh]=简体", "zh_TW.UTF-8", "繁體"),
            ("Name[es]=Español", "de_DE.UTF-8", "Base"),
            ("", "es_CL.UTF-8", "Base"),
            ("Name[C]=wrong", "C.UTF-8", "Base"),
            ("Name[POSIX]=wrong", "POSIX", "Base"),
            ("Name[es]=Español", "", "Base"),
        ] {
            let ini = Ini::parse(&format!("[Desktop Entry]\nName=Base\n{translations}"));
            assert_eq!(
                ini.localized("Desktop Entry", "Name", locale),
                Some(expected),
                "{locale}: {translations}"
            );
        }
    }
    #[test]
    fn annotations_do_not_merge_localized_and_base_keys() {
        let mut ini =
            Ini::parse("[Desktop Entry]\nName[$i]=Base\nName[es][$i]=Español\nIcon=base-icon");
        ini.merge(Ini::parse("[Desktop Entry]\nName=Changed\nName[es]=Changed\nName[zh_TW]=繁體\nIcon[es][$d]=ignored"));
        assert_eq!(ini.get("Desktop Entry", "Name"), Some("Base"));
        assert_eq!(
            ini.localized("Desktop Entry", "Name", "es_CL"),
            Some("Español")
        );
        assert_eq!(
            ini.localized("Desktop Entry", "Name", "zh_TW"),
            Some("繁體")
        );
        assert_eq!(
            ini.localized("Desktop Entry", "Icon", "es_CL"),
            Some("base-icon")
        );
    }
    #[test]
    fn localized_config_siblings_cannot_override_settings() {
        let root = crate::history::test_dir();
        for (name, text) in [
            (
                "plasma-visual-screensaverrc",
                "[General]\nIdleMinutes[$i]=1\nIdleMinutes[es]=20",
            ),
            (
                "powerdevilrc",
                "[AC][Display][$i]\nDimDisplayWhenIdle=true\nDimDisplayWhenIdle[es]=false\n\
                 DimDisplayIdleTimeoutSec=10\nDimDisplayIdleTimeoutSec[es]=999\n\
                 TurnOffDisplayWhenIdle=true\nTurnOffDisplayWhenIdle[es]=false\n\
                 TurnOffDisplayIdleTimeoutSec=20\nTurnOffDisplayIdleTimeoutSec[es]=999\n\
                 LockBeforeTurnOffDisplay=true\nLockBeforeTurnOffDisplay[es]=false\n\
                 [AC][SuspendAndShutdown][$i]\nAutoSuspendAction=1\nAutoSuspendAction[es]=0\n\
                 AutoSuspendIdleTimeoutSec=25\nAutoSuspendIdleTimeoutSec[es]=999",
            ),
            (
                "kscreenlockerrc",
                "[Daemon]\nTimeout[$i]=0.25\nTimeout[es]=20\nAutolock=true\nAutolock[es]=false",
            ),
        ] {
            std::fs::write(root.path().join(name), text).unwrap();
        }
        let config = Config::read_sources(&[root.path().into()], true, false, Some(Profile::Ac));
        assert_eq!(config.timeout, 60);
        assert_eq!(
            config
                .conflicts
                .iter()
                .map(|c| (c.setting.as_str(), c.seconds))
                .collect::<Vec<_>>(),
            [
                ("dim", 10),
                ("lock", 15),
                ("screen-off", 20),
                ("suspend", 25)
            ]
        );
        // With auto-lock disabled, the remaining lock comes from PowerDevil's
        // LockBeforeTurnOffDisplay; neither boolean's localized sibling applies.
        std::fs::write(
            root.path().join("kscreenlockerrc"),
            "[Daemon]\nTimeout=0.25\nAutolock=false\nAutolock[es]=true",
        )
        .unwrap();
        let config = Config::read_sources(&[root.path().into()], true, false, Some(Profile::Ac));
        assert_eq!(
            config
                .conflicts
                .iter()
                .find(|c| c.setting == "lock")
                .unwrap()
                .seconds,
            20
        );
    }
    #[test]
    fn cascade_inherits_and_overrides_all_sources() {
        let root = crate::history::test_dir();
        let low = root.path().join("low");
        let system = root.path().join("system");
        let user = root.path().join("user");
        for dir in [&low, &system, &user] {
            std::fs::create_dir(dir).unwrap();
        }
        let dirs = vec![low.clone(), system.clone(), user.clone()];
        std::fs::write(
            low.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=3",
        )
        .unwrap();
        std::fs::write(
            system.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=1",
        )
        .unwrap();
        std::fs::write(
            system.join("powerdevilrc"),
            "[AC][Display]\nDimDisplayIdleTimeoutSec=15",
        )
        .unwrap();
        std::fs::write(system.join("kscreenlockerrc"), "[Daemon]\nTimeout=0.5").unwrap();
        let read = || Config::read_sources(&dirs, true, false, Some(Profile::Ac));
        let config = read();
        assert_eq!(config.timeout, 60);
        assert_eq!(
            config
                .conflicts
                .iter()
                .map(|c| c.seconds)
                .collect::<Vec<_>>(),
            [15, 30]
        );
        std::fs::write(
            user.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=2",
        )
        .unwrap();
        assert_eq!(read().timeout, 120);
    }
    #[test]
    fn immutable_keys_and_groups_win_over_user_overrides() {
        for system in [
            "[General][$i]\nIdleMinutes=1",
            "[General]\nIdleMinutes[$i]=1",
        ] {
            let mut ini = Ini::default();
            ini.merge(Ini::parse(system));
            ini.merge(Ini::parse("[General]\nIdleMinutes=20\nOther=added"));
            assert_eq!(ini.get("General", "IdleMinutes"), Some("1"));
        }
        let mut ini = Ini::parse("[AC][$i]\n[AC][Display]\nDimDisplayIdleTimeoutSec=10");
        ini.merge(Ini::parse("[AC][Display]\nDimDisplayIdleTimeoutSec=99"));
        assert_eq!(
            ini.get("AC/Display", "DimDisplayIdleTimeoutSec"),
            Some("99") // Parent immutability does not lock a nested group.
        );
        let mut ini = Ini::parse("[General]\nIdleMinutes=2");
        ini.merge(Ini::parse("[General]\nIdleMinutes[$d]=\nOther[$i]=yes"));
        assert_eq!(ini.get("General", "IdleMinutes"), None);
        assert_eq!(ini.get("General", "Other"), Some("yes"));
    }
    #[test]
    fn defaults_follow_each_profile() {
        for (profile, expected) in [
            (Profile::Ac, (300, 600, 900)),
            (Profile::Battery, (120, 300, 600)),
            (Profile::LowBattery, (60, 120, 300)),
        ] {
            let c = Config::parse(
                &Ini::parse("[General]\nIdleMinutes=240"),
                &Ini::default(),
                &Ini::parse("[Daemon]\nAutolock=false"),
                true,
                false,
                Some(profile),
            );
            let seconds = |setting| {
                c.conflicts
                    .iter()
                    .find(|c| c.setting == setting)
                    .unwrap()
                    .seconds
            };
            assert_eq!(
                (seconds("dim"), seconds("screen-off"), seconds("suspend")),
                expected
            );
        }
    }
    #[test]
    fn reads_only_active_profile_and_preserves_global_lock() {
        let p = Ini::parse(
            "[AC][Display]\nDimDisplayIdleTimeoutSec=5\n[Battery][Display]\nDimDisplayWhenIdle=false\nTurnOffDisplayIdleTimeoutSec=20\nLockBeforeTurnOffDisplay=true\n[Battery][SuspendAndShutdown]\nAutoSuspendAction=0",
        );
        let c = Config::parse(
            &Ini::default(),
            &p,
            &Ini::default(),
            true,
            false,
            Some(Profile::Battery),
        );
        assert_eq!(
            c.conflicts
                .iter()
                .map(|c| (c.setting.as_str(), c.seconds))
                .collect::<Vec<_>>(),
            [("lock", 20), ("screen-off", 20)]
        );
        let c = Config::parse(&Ini::default(), &p, &Ini::default(), true, false, None);
        assert_eq!(
            c.conflicts
                .iter()
                .map(|c| c.setting.as_str())
                .collect::<Vec<_>>(),
            ["lock"]
        );
        assert_eq!(Profile::from_id("performance"), None);
    }
    #[test]
    fn defaults_and_equal_time() {
        let c = Config::parse(
            &Ini::default(),
            &Ini::default(),
            &Ini::default(),
            true,
            false,
            Some(Profile::Ac),
        );
        assert_eq!(c.timeout, 600);
        assert_eq!(
            c.conflicts
                .iter()
                .map(|v| v.setting.as_str())
                .collect::<Vec<_>>(),
            ["lock", "dim", "screen-off"]
        );
        assert_eq!(c.conflicts[0].seconds, 300);
    }
    #[test]
    fn disabled_and_negative_timeouts() {
        let p = Ini::parse(
            "[AC][Display]\nDimDisplayWhenIdle=false\nTurnOffDisplayIdleTimeoutSec=-1\n[AC][SuspendAndShutdown]\nAutoSuspendAction=0",
        );
        let c = Config::parse(
            &Ini::parse("[General]\nIdleMinutes=1"),
            &p,
            &Ini::parse("[Daemon]\nAutolock=false"),
            true,
            false,
            Some(Profile::Ac),
        );
        assert!(c.conflicts.is_empty());
        assert_eq!(c.timeout, 60);
    }
    #[test]
    fn fractional_lock_and_vm() {
        let c = Config::parse(
            &Ini::parse("[General]\nIdleMinutes=240"),
            &Ini::default(),
            &Ini::parse("[Daemon]\nTimeout=0.5"),
            true,
            true,
            Some(Profile::Ac),
        );
        assert_eq!(c.conflicts[0].seconds, 30);
        assert!(!c.conflicts.iter().any(|c| c.setting == "suspend"));
    }
    #[test]
    fn expanded_path_aliases_use_xdg_defaults_and_ignore_invalid_values() {
        for (alias, key, suffix) in [
            ("QT_CONFIG_HOME", "XDG_CONFIG_HOME", ".config"),
            ("QT_DATA_HOME", "XDG_DATA_HOME", ".local/share"),
            ("QT_CACHE_HOME", "XDG_CACHE_HOME", ".cache"),
            ("XDG_STATE_HOME", "XDG_STATE_HOME", ".local/state"),
        ] {
            for invalid in ["", "relative"] {
                assert_eq!(
                    expand_from(&format!("${alias}"), |name| match name {
                        "HOME" => Some("/home/test".into()),
                        n if n == alias || n == key => Some(invalid.into()),
                        _ => None,
                    }),
                    format!("/home/test/{suffix}")
                );
            }
            assert_eq!(
                expand_from(&format!("${alias}"), |name| (name == alias)
                    .then(|| "/custom".into())),
                "/custom"
            );
        }
        for key in ["HOME", "XDG_RUNTIME_DIR"] {
            for invalid in ["", "relative"] {
                assert_eq!(
                    expand_from(&format!("${key}"), |name| (name == key)
                        .then(|| invalid.into())),
                    ""
                );
            }
        }
    }
    #[test]
    fn expanded_search_lists_filter_empty_and_relative_entries() {
        for (key, default) in [
            ("XDG_CONFIG_DIRS", "/etc/xdg"),
            ("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
        ] {
            assert_eq!(expand_from(&format!("${key}"), |_| None), default);
            assert_eq!(
                expand_from(&format!("${key}"), |_| Some("".into())),
                default
            );
            assert_eq!(
                expand_from(&format!("${key}"), |_| Some(":relative:/one::/two:".into())),
                "/one:/two"
            );
        }
    }
}
