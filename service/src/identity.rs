// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::config::Ini;
use std::{
    collections::HashSet,
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
const GENERIC_ICON: &str = "application-x-executable";

fn basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}
fn desktop_id(value: &str) -> String {
    let value = value.to_lowercase();
    value.strip_suffix(".desktop").unwrap_or(&value).to_owned()
}

/// Tokenize only; never execute desktop commands or expand their environment.
fn exec_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for c in line.chars() {
        if escaped {
            word.push(c);
            escaped = false;
        } else if c == '\\' && quote != Some('\'') {
            started = true;
            escaped = true;
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            started = true;
            quote = Some(c);
        } else if quote.is_none() && c.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            started = true;
            word.push(c);
        }
    }
    if escaped || quote.is_some() {
        return None;
    }
    if started {
        words.push(word);
    }
    Some(words)
}

fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(key, _)| {
        !key.is_empty()
            && key
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
    })
}

// Option arities checked against the installed --help/man pages (versions and
// supported subsets are recorded in docs/service-dbus.md). Names before `|` are
// short spellings; the last name is the canonical key for repeated overrides.
#[derive(Clone, Copy)]
enum Argument {
    Flag,
    Required,
    Optional,
    EqualsOnly,
}
struct OptionSpec {
    names: &'static str,
    argument: Argument,
}
impl OptionSpec {
    fn key(&self) -> &'static str {
        self.names.rsplit('|').next().unwrap()
    }
    fn matches(&self, name: &str) -> bool {
        self.names.split('|').any(|alias| alias == name)
    }
}
macro_rules! options {
    ($($kind:ident: [$($names:literal),* $(,)?]),* $(,)?) => {
        &[$($(OptionSpec { names: $names, argument: Argument::$kind },)*)*]
    };
}
#[derive(Clone, Copy, PartialEq)]
enum Operands {
    Command,
    Env,
    Nice,
    Taskset,
    Flatpak,
    Snap,
}
struct LauncherSpec {
    name: &'static str,
    options: &'static [OptionSpec],
    operands: Operands,
    unsupported: &'static [&'static str],
    terminator: bool,
}
const LAUNCHERS: &[LauncherSpec] = &[
    LauncherSpec {
        name: "env",
        options: options! {
            Flag: ["i|ignore-environment", "0|null", "v|debug", "list-signal-handling", "h|help", "V|version"],
            Required: ["C|chdir", "f|file", "u|unset", "S|split-string", "a|argv0"],
            Optional: ["ignore-signal", "default-signal", "block-signal"],
        },
        operands: Operands::Env,
        unsupported: &["null", "split-string"],
        terminator: true,
    },
    LauncherSpec {
        name: "nice",
        options: options! { Required: ["n|adjustment"], Flag: ["h|help", "V|version"] },
        operands: Operands::Nice,
        unsupported: &[],
        terminator: true,
    },
    LauncherSpec {
        name: "ionice",
        options: options! {
            Required: ["c|class", "n|classdata", "p|pid", "P|pgid", "u|uid"],
            Flag: ["t|ignore", "h|help", "V|version"],
        },
        operands: Operands::Command,
        unsupported: &["pid", "pgid", "uid"],
        terminator: true,
    },
    LauncherSpec {
        name: "dbus-run-session",
        options: options! { Required: ["dbus-daemon", "config-file"], Flag: ["help", "version"] },
        operands: Operands::Command,
        unsupported: &[],
        terminator: true,
    },
    LauncherSpec {
        name: "dbus-launch",
        options: options! {
            Flag: ["sh-syntax", "csh-syntax", "auto-syntax", "binary-syntax", "close-stderr", "exit-with-session", "exit-with-x11", "help", "version"],
            EqualsOnly: ["autolaunch", "config-file"],
        },
        operands: Operands::Command,
        unsupported: &["autolaunch"],
        // This tool has its own parser, not getopt; its synopsis has no --.
        terminator: false,
    },
    LauncherSpec {
        name: "setsid",
        options: options! { Flag: ["c|ctty", "f|fork", "w|wait", "h|help", "V|version"] },
        operands: Operands::Command,
        unsupported: &[],
        terminator: true,
    },
    LauncherSpec {
        name: "nohup",
        options: options! { Flag: ["h|help", "V|version"] },
        operands: Operands::Command,
        unsupported: &[],
        terminator: true,
    },
    LauncherSpec {
        name: "systemd-run",
        options: options! {
            Flag: ["h|help", "version", "no-ask-password", "user", "scope", "slice-inherit", "no-block", "r|remain-after-exit", "wait", "send-sighup", "d|same-dir", "R|same-root-dir", "t|pty", "T|pty-late", "P|pipe", "q|quiet", "v|verbose", "G|collect", "S|shell", "ignore-failure", "no-pager", "on-timezone-change", "on-clock-change"],
            Required: ["H|host", "M|machine", "u|unit", "p|property", "description", "slice", "expand-environment", "service-type", "uid", "gid", "nice", "working-directory", "root-directory", "E|setenv", "json", "job-mode", "background", "path-property", "socket-property", "on-active", "on-boot", "on-startup", "on-unit-active", "on-unit-inactive", "on-calendar", "timer-property"],
        },
        operands: Operands::Command,
        unsupported: &["shell"],
        terminator: true,
    },
    LauncherSpec {
        name: "taskset",
        options: options! { Flag: ["a|all-tasks", "p|pid", "c|cpu-list", "h|help", "V|version"] },
        operands: Operands::Taskset,
        unsupported: &["pid"],
        terminator: true,
    },
    LauncherSpec {
        name: "flatpak",
        options: options! {
            Flag: ["h|help", "help-all", "u|user", "system", "d|devel", "log-session-bus", "log-system-bus", "log-a11y-bus", "no-a11y-bus", "a11y-bus", "no-session-bus", "session-bus", "no-documents-portal", "file-forwarding", "sandbox", "p|die-with-parent", "parent-expose-pids", "parent-share-pids", "v|verbose", "ostree-verbose"],
            Required: ["installation", "arch", "command", "cwd", "branch", "runtime", "runtime-version", "commit", "runtime-commit", "parent-pid", "instance-id-fd", "app-path", "app-fd", "usr-path", "usr-fd", "bind-fd", "ro-bind-fd", "share", "unshare", "socket", "nosocket", "device", "nodevice", "allow", "disallow", "filesystem", "nofilesystem", "env", "env-fd", "unset-env", "own-name", "talk-name", "no-talk-name", "system-own-name", "system-talk-name", "system-no-talk-name", "a11y-own-name", "add-policy", "remove-policy", "usb", "nousb", "usb-list", "usb-list-file", "persist"],
        },
        operands: Operands::Flatpak,
        unsupported: &["help-all"],
        terminator: true,
    },
    LauncherSpec {
        name: "snap",
        options: options! {
            Flag: ["shell", "debug-log", "trace-exec"],
            // Debugger argument conventions and undocumented internal options
            // are deliberately unsupported, rather than guessed.
            EqualsOnly: ["strace", "gdbserver"],
        },
        operands: Operands::Snap,
        unsupported: &["shell", "strace", "gdbserver"],
        terminator: true,
    },
];

struct ParsedOptions<'a> {
    rest: &'a [String],
    values: std::collections::HashMap<&'static str, Option<&'a str>>,
}
impl<'a> ParsedOptions<'a> {
    fn has(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }
    fn value(&self, key: &str) -> Option<&'a str> {
        self.values.get(key).copied().flatten()
    }
}

/// One non-permuting getopt grammar. Stop at the command so its arguments never
/// become launcher options. Optional long operands must be attached with `=`;
/// optional short operands, if added to a spec, must likewise be attached.
/// Abbreviated long names are intentionally rejected even when a tool accepts
/// them: their meaning can change when that tool adds another option.
fn launcher_args<'a>(spec: &LauncherSpec, mut args: &'a [String]) -> Option<ParsedOptions<'a>> {
    let mut values = std::collections::HashMap::new();
    while let Some((arg, tail)) = args.split_first() {
        if arg == "--" {
            if !spec.terminator {
                return None;
            }
            args = tail;
            break;
        }
        if spec.operands == Operands::Env {
            if arg == "-" {
                values.insert("ignore-environment", None);
                args = tail;
                continue;
            }
            // env accepts environment names beyond shell identifier syntax.
            if arg.contains('=') && !arg.starts_with('-') {
                if arg.starts_with('=') {
                    return None;
                }
                args = tail;
                continue;
            }
        }
        if spec.operands == Operands::Nice
            && let Some(number) = arg.strip_prefix('-')
            && number.parse::<i32>().is_ok()
        {
            // Historical -N / --N / -+N forms: one leading '-' is syntax.
            values.insert("adjustment", Some(number));
            args = tail;
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            let option = spec.options.iter().find(|o| o.key() == name)?;
            let (value, remaining) = option_value(option.argument, attached, tail)?;
            values.insert(option.key(), value);
            args = remaining;
        } else if let Some(shorts) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            args = tail;
            for (offset, short) in shorts.char_indices() {
                let name = short.to_string();
                let option = spec.options.iter().find(|o| o.matches(&name))?;
                let suffix = &shorts[offset + short.len_utf8()..];
                let attached = (!suffix.is_empty()).then_some(suffix);
                if matches!(option.argument, Argument::Flag) {
                    values.insert(option.key(), None);
                } else {
                    let (value, remaining) = option_value(option.argument, attached, args)?;
                    values.insert(option.key(), value);
                    args = remaining;
                    break;
                }
            }
        } else {
            break;
        }
    }
    let parsed = ParsedOptions { rest: args, values };
    if ["help", "version"]
        .into_iter()
        .chain(spec.unsupported.iter().copied())
        .any(|key| parsed.has(key))
    {
        return None;
    }
    Some(parsed)
}
fn option_value<'a>(
    kind: Argument,
    attached: Option<&'a str>,
    tail: &'a [String],
) -> Option<(Option<&'a str>, &'a [String])> {
    let (value, rest) = match kind {
        Argument::Flag => return attached.is_none().then_some((None, tail)),
        Argument::Optional => (attached, tail),
        Argument::EqualsOnly => (Some(attached?), tail),
        Argument::Required => match attached {
            Some(value) => (Some(value), tail),
            None => {
                let (value, rest) = tail.split_first()?;
                (Some(value.as_str()), rest)
            }
        },
    };
    // Reject empty operands conservatively, even for options that allow them.
    if value.is_some_and(str::is_empty) {
        return None;
    }
    Some((value, rest))
}

fn cpu_operand(value: &str, list: bool) -> bool {
    if !list {
        let mask = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);
        return !mask.is_empty() && mask.chars().all(|c| c.is_ascii_hexdigit());
    }
    value.split(',').all(|part| {
        let (range, stride) = part
            .split_once(':')
            .map_or((part, None), |(r, s)| (r, Some(s)));
        let number = |s: &str| {
            (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
                .then(|| s.parse::<u32>().ok())
                .flatten()
        };
        if let Some((low, high)) = range.split_once('-') {
            matches!((number(low), number(high)), (Some(l), Some(h)) if l <= h)
                && stride.is_none_or(|s| number(s).is_some_and(|n| n > 0))
        } else {
            stride.is_none() && number(range).is_some()
        }
    })
}

fn executable(value: &str) -> Option<String> {
    let binary = basename(value);
    (!binary.is_empty() && !binary.starts_with('-') && !value.contains('%') && !value.contains('='))
        .then(|| binary.to_owned())
}
fn flatpak_app(reference: &str) -> Option<&str> {
    let mut parts = reference.split('/');
    let app = parts.next()?;
    let components: Vec<_> = app.split('.').collect();
    // A ref is APP_ID[/ARCH[/BRANCH]], never a filesystem basename. Reject
    // full app/runtime refs, paths, empty components and excess suffixes.
    if app.len() > 255
        || components.len() < 3
        || components.iter().enumerate().any(|(index, part)| {
            part.is_empty()
                || !part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                || !part.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || c == '_'
                        || (c == '-' && index == components.len() - 1)
                })
        })
    {
        return None;
    }
    let suffixes: Vec<_> = parts.collect();
    if suffixes.len() > 2
        || suffixes.iter().any(|s| {
            s.is_empty()
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        })
    {
        return None;
    }
    Some(app)
}

/// Recognize only launchers whose option/operand boundaries are specified.
/// Unsupported wrappers remain blocked here, rather than indexed as programs.
fn exec_aliases(line: &str) -> Vec<String> {
    exec_words(line)
        .and_then(|words| command_aliases(&words))
        .unwrap_or_default()
}
fn command_aliases(words: &[String]) -> Option<Vec<String>> {
    let mut rest = words;
    while rest.first().is_some_and(|word| assignment(word)) {
        rest = &rest[1..];
    }
    while let Some((command, args)) = rest.split_first() {
        let binary = basename(command);
        if matches!(
            binary,
            "optirun"
                | "mangohud"
                | "gamemoderun"
                | "primusrun"
                | "prime-run"
                | "steam-run"
                | "sh"
                | "bash"
                | "dash"
                | "zsh"
                | "ksh"
                | "fish"
                | "csh"
                | "tcsh"
        ) {
            return None;
        }
        let Some(spec) = LAUNCHERS.iter().find(|s| s.name == binary) else {
            return Some(vec![executable(command)?]);
        };
        let args = if matches!(spec.operands, Operands::Flatpak | Operands::Snap) {
            let (subcommand, args) = args.split_first()?;
            if subcommand != "run" {
                return None;
            }
            args
        } else {
            args
        };
        let parsed = launcher_args(spec, args)?;
        rest = parsed.rest;
        match spec.operands {
            Operands::Env => {
                while rest
                    .first()
                    .is_some_and(|s| s.contains('=') && !s.starts_with('-'))
                {
                    if rest[0].starts_with('=') {
                        return None;
                    }
                    rest = &rest[1..];
                }
            }
            Operands::Nice => {
                if parsed
                    .value("adjustment")
                    .is_some_and(|v| v.parse::<i32>().is_err())
                {
                    return None;
                }
            }
            Operands::Taskset => {
                let (cpu, command) = rest.split_first()?;
                if !cpu_operand(cpu, parsed.has("cpu-list")) {
                    return None;
                }
                rest = command;
            }
            Operands::Flatpak | Operands::Snap => {
                let app = rest.first()?;
                let app = if spec.operands == Operands::Flatpak {
                    flatpak_app(app)?
                } else {
                    // Snap app names are identifiers, not paths.
                    let parts: Vec<_> = app.split('.').collect();
                    if parts.len() > 2
                        || parts.iter().any(|part| {
                            !part.starts_with(|c: char| c.is_ascii_alphanumeric())
                                || !part.ends_with(|c: char| c.is_ascii_alphanumeric())
                                || !part.chars().all(|c| {
                                    c.is_ascii_lowercase()
                                        || c.is_ascii_digit()
                                        || matches!(c, '-' | '_')
                                })
                        })
                    {
                        return None;
                    }
                    app
                };
                let mut aliases = Vec::new();
                if let Some(command) = parsed.value("command") {
                    aliases.push(executable(command)?);
                }
                aliases.push(app.to_owned());
                return Some(aliases);
            }
            Operands::Command => {}
        }
        if binary == "ionice"
            && (parsed.value("class").is_some_and(|v| {
                !matches!(
                    v,
                    "0" | "1" | "2" | "3" | "none" | "realtime" | "best-effort" | "idle"
                )
            }) || parsed
                .value("classdata")
                .is_some_and(|v| !v.parse::<u8>().is_ok_and(|n| n <= 7)))
        {
            return None;
        }
    }
    None
}

struct DesktopEntry {
    id: String,
    name: String,
    base_name: String,
    icon: String,
    base_icon: String,
    class: String,
    executables: Vec<String>,
    unlisted: bool,
}
impl DesktopEntry {
    fn identity(&self) -> (String, String) {
        (
            self.name.clone(),
            if self.icon.is_empty() {
                GENERIC_ICON.into()
            } else {
                self.icon.clone()
            },
        )
    }
}
#[derive(Default)]
pub struct DesktopIndex {
    entries: Vec<DesktopEntry>,
    ids: HashSet<String>,
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
        // Stable alias collisions, independent of filesystem enumeration order.
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.path());
        for e in entries {
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
            let id = desktop_id(
                &path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('/', "-"),
            );
            // XDG precedence applies to the entire entry, including its aliases.
            if !self.ids.insert(id.clone()) {
                continue;
            }
            let get = |key| ini.get("Desktop Entry", key).unwrap_or_default().to_owned();
            self.entries.push(DesktopEntry {
                id,
                name: name.into(),
                base_name: get("Name"),
                icon: ini
                    .localized("Desktop Entry", "Icon", locale)
                    .filter(|icon| !icon.is_empty())
                    .unwrap_or_default()
                    .into(),
                base_icon: get("Icon"),
                class: get("StartupWMClass"),
                executables: exec_aliases(&get("Exec")),
                unlisted: ini.boolean("Desktop Entry", "NoDisplay", false)
                    || ini.boolean("Desktop Entry", "Hidden", false),
            });
        }
    }
    fn lookup(&self, candidates: &[&str]) -> Option<(String, String)> {
        // An absolute executable can also name a desktop ID whose Exec uses a
        // different launcher. Keep the original hint and the existing rule order.
        let candidates: Vec<_> = candidates
            .iter()
            .flat_map(|candidate| {
                std::iter::once(*candidate).chain(
                    Path::new(candidate)
                        .is_absolute()
                        .then(|| basename(candidate)),
                )
            })
            .collect();
        for rule in 0..5 {
            for candidate in candidates.iter().filter(|s| !s.is_empty()) {
                let matches = |entry: &&DesktopEntry| match rule {
                    0 => entry.id == desktop_id(candidate),
                    1 => entry.class.eq_ignore_ascii_case(candidate),
                    2 => entry
                        .executables
                        .iter()
                        .any(|exe| exe.eq_ignore_ascii_case(basename(candidate))),
                    3 => {
                        entry.name.to_lowercase() == candidate.to_lowercase()
                            || entry.base_name.to_lowercase() == candidate.to_lowercase()
                    }
                    4 => {
                        entry.icon.eq_ignore_ascii_case(candidate)
                            || entry.base_icon.eq_ignore_ascii_case(candidate)
                    }
                    _ => unreachable!(),
                };
                if let Some(entry) = self
                    .entries
                    .iter()
                    .filter(matches)
                    .min_by_key(|entry| entry.unlisted)
                {
                    return Some(entry.identity());
                }
            }
        }
        None
    }
    pub fn resolve(
        &self,
        desktop: &str,
        app_id: &str,
        class: &str,
        executable: &str,
    ) -> (String, String) {
        if let Some(identity) = self.lookup(&[desktop, app_id, class, executable]) {
            return identity;
        }
        let name = [class, app_id, basename(executable)]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("Unknown application");
        (name.into(), GENERIC_ICON.into())
    }
    pub fn policy(&self, who: &str) -> (String, String) {
        self.lookup(&[who])
            .unwrap_or_else(|| (who.into(), GENERIC_ICON.into()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path, id: &str, fields: &str) {
        let path = root.join("applications").join(format!("{id}.desktop"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("[Desktop Entry]\n{fields}\n")).unwrap();
    }
    fn expect(index: &DesktopIndex, who: &str, name: &str, icon: &str) {
        assert_eq!(index.policy(who), (name.into(), icon.into()), "{who}");
        // Each bridge identity hint also uses the same matching rules.
        for hints in [
            (who, "", "", ""),
            ("", who, "", ""),
            ("", "", who, ""),
            ("", "", "", who),
        ] {
            assert_eq!(
                index.resolve(hints.0, hints.1, hints.2, hints.3),
                (name.into(), icon.into()),
                "{hints:?}"
            );
        }
    }
    #[test]
    fn who_strings_match_desktop_class_exec_names_and_icons() {
        let root = crate::history::test_dir();
        for (id, fields) in [
            (
                "com.anthropic.Claude",
                "Name=Claude\nExec=/usr/lib/claude-desktop/claude-desktop %U\nStartupWMClass=com.anthropic.Claude\nIcon=claude-desktop",
            ),
            (
                "google-chrome",
                "Name=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\nIcon=google-chrome\nNoDisplay=true",
            ),
            (
                "org.example.Game",
                "Name=Example Game\nStartupWMClass=My SDL application\nExec=/opt/game/bin/game\nIcon=example-game",
            ),
            (
                "org.example.Overlay",
                "Name=Steam Overlay\nExec=/opt/steam/gameoverlayui\nIcon=steam",
            ),
            (
                "org.kde.elisa",
                "Name=Elisa\nName[es]=Elisa Música\nExec=elisa %U\nIcon=elisa-icon",
            ),
            (
                "nested/monitor",
                "Name=System Monitor\nName[es]=Monitor del sistema\nIcon=monitor-icon\nIcon[es]=monitor-es",
            ),
            ("org.example.NoIcon", "Name=No Icon"),
        ] {
            fixture(root.path(), id, fields);
        }
        let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], "es_CL.UTF-8");
        for who in [
            "com.anthropic.Claude",
            "COM.ANTHROPIC.CLAUDE.DESKTOP",
            "claude-desktop",
            "/usr/lib/claude-desktop/claude-desktop",
        ] {
            expect(&index, who, "Claude", "claude-desktop");
        }
        expect(
            &index,
            "google-chrome-stable",
            "Google Chrome",
            "google-chrome",
        );
        expect(&index, "My SDL application", "Example Game", "example-game");
        expect(&index, "gameoverlayui", "Steam Overlay", "steam");
        for who in ["Elisa", "Elisa Música"] {
            expect(&index, who, "Elisa Música", "elisa-icon");
        }
        for who in [
            "NESTED-MONITOR.DESKTOP",
            "System Monitor",
            "Monitor del sistema",
            "monitor-icon",
            "monitor-es",
        ] {
            expect(&index, who, "Monitor del sistema", "monitor-es");
        }
        expect(&index, "No Icon", "No Icon", GENERIC_ICON);
        for who in [
            "Unregistered pretty name",
            "/opt/unknown/application",
            GENERIC_ICON,
            "",
        ] {
            assert_eq!(index.policy(who), (who.into(), GENERIC_ICON.into()));
        }
        // StartupWMClass works even when Exec and Icon cannot identify Claude.
        fixture(
            root.path(),
            "com.anthropic.Claude",
            "Name=Claude\nExec=launcher\nStartupWMClass=claude-desktop\nIcon=claude",
        );
        let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], "C");
        expect(&index, "CLAUDE-DESKTOP", "Claude", "claude");
    }
    #[test]
    fn exec_launchers_use_the_app_not_wrappers_or_arguments() {
        for (exec, aliases) in [
            (
                "/usr/lib/claude-desktop/claude-desktop %U",
                vec!["claude-desktop"],
            ),
            (
                r#"env -u OLD --chdir /opt FLAG=1 /usr/bin/google-chrome-stable %U"#,
                vec!["google-chrome-stable"],
            ),
            (
                r#"/usr/bin/env --ignore-environment --unset=OLD FLAG="two words" setsid -fw "/opt/My App/bin/my-app" --document %f"#,
                vec!["my-app"],
            ),
            (
                "FLAG=1 nice -n 5 ionice -t -c 2 -n 4 nohup /opt/gameoverlayui %U",
                vec!["gameoverlayui"],
            ),
            ("dbus-run-session -- /usr/bin/elisa %U", vec!["elisa"]),
            (
                "/usr/bin/flatpak run --branch stable --arch=x86_64 --command=claude-desktop --file-forwarding com.anthropic.Claude @@u %U @@",
                vec!["claude-desktop", "com.anthropic.Claude"],
            ),
            (
                "env A=1 flatpak run --command /app/bin/elisa -- org.kde.elisa %U",
                vec!["elisa", "org.kde.elisa"],
            ),
            ("/usr/bin/snap run chromium %U", vec!["chromium"]),
            (
                "env A=1 snap run --debug-log music.player %U",
                vec!["music.player"],
            ),
            (
                r#""/opt/My App/bin/app with spaces" %U"#,
                vec!["app with spaces"],
            ),
        ] {
            let root = crate::history::test_dir();
            fixture(
                root.path(),
                "org.test.Fixture",
                &format!("Name=Fixture\nIcon=fixture\nExec={exec}"),
            );
            let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], "C");
            for alias in aliases {
                expect(&index, alias, "Fixture", "fixture");
            }
            for wrapper in [
                "env",
                "flatpak",
                "snap",
                "gamemoderun",
                "mangohud",
                "nice",
                "ionice",
                "nohup",
                "dbus-run-session",
                "%U",
                "--document",
            ] {
                assert_eq!(
                    index.policy(wrapper),
                    (wrapper.into(), GENERIC_ICON.into()),
                    "{exec}: {wrapper}"
                );
            }
        }
        for exec in [
            "",
            "env",
            "flatpak install org.example.App",
            "snap list",
            "flatpak run --command",
            "env -u",
            "\"unterminated",
        ] {
            assert!(exec_aliases(exec).is_empty(), "{exec}");
        }
    }
    #[test]
    fn launcher_documented_forms() {
        // Examples exercise each supported launcher and every argument convention.
        for (exec, aliases) in [
            (
                "env -iv -uOLD -C/opt -aold --argv0=new A=1 /opt/app -u ignored",
                vec!["app"],
            ),
            ("env - --unset OLD A-B=value /opt/app", vec!["app"]),
            ("env A=1 --unset=OLD B=2 /opt/app", vec!["app"]),
            (
                "env --ignore-signal --block-signal=INT --default-signal=TERM -- /opt/app",
                vec!["app"],
            ),
            ("env -i -- A=1 /opt/app", vec!["app"]),
            (
                "env --file=/opt/settings --list-signal-handling /opt/app",
                vec!["app"],
            ),
            (
                "nice -n5 --adjustment=6 -n 7 -- /opt/app -n invalid",
                vec!["app"],
            ),
            ("nice -5 /opt/app", vec!["app"]),
            ("nice --5 /opt/app", vec!["app"]),
            ("nice -+5 /opt/app", vec!["app"]),
            ("ionice -tc2 -n4 --class=idle -- /opt/app", vec!["app"]),
            (
                "ionice --ignore --class best-effort --classdata=4 /opt/app",
                vec!["app"],
            ),
            (
                "dbus-run-session --dbus-daemon=/bin/old --dbus-daemon /bin/new --config-file /opt/config -- /opt/app",
                vec!["app"],
            ),
            (
                "dbus-launch --close-stderr --exit-with-session --config-file=/opt/config /opt/app",
                vec!["app"],
            ),
            (
                "dbus-launch --sh-syntax --csh-syntax --auto-syntax --binary-syntax --exit-with-x11 /opt/app",
                vec!["app"],
            ),
            ("setsid -cfw --fork -- /opt/app -f", vec!["app"]),
            ("nohup -- /opt/app --help", vec!["app"]),
            (
                "systemd-run --user --scope -qGd -uold --unit=new -pCPUWeight=10 -EKEY=value --expand-environment=no -- /opt/app --unit=argument",
                vec!["app"],
            ),
            (
                "systemd-run --on-active=5 --timer-property AccuracySec=1 --socket-property ListenStream=12345 --path-property PathExists=/opt/file /opt/app",
                vec!["app"],
            ),
            (
                "systemd-run --working-directory /opt --nice=5 -Hhost -Mmachine /opt/app",
                vec!["app"],
            ),
            ("taskset -c -- 0,1 /opt/app", vec!["app"]),
            ("taskset -c -a 0,1 /opt/app", vec!["app"]),
            ("taskset -ac --cpu-list 0-31:2,33 /opt/app", vec!["app"]),
            ("taskset -- ff /opt/app", vec!["app"]),
            ("taskset -aa 0x03 /opt/app", vec!["app"]),
            (
                "flatpak run -udpv --arch=x86_64 --branch stable --command=old --command=/app/bin/new -- org.example.App/x86_64/stable %U",
                vec!["new", "org.example.App"],
            ),
            (
                "flatpak run --command /app/bin/old --command=new org.example.App/x86_64",
                vec!["new", "org.example.App"],
            ),
            (
                "flatpak run --commit abc --runtime-commit=def --instance-id-fd 3 org.example.App",
                vec!["org.example.App"],
            ),
            (
                "flatpak run --arch=old --arch=new org.example.App",
                vec!["org.example.App"],
            ),
            (
                "flatpak run org.example.App --command=argument",
                vec!["org.example.App"],
            ),
            (
                "snap run --debug-log --trace-exec --debug-log -- music.player %U",
                vec!["music.player"],
            ),
            ("snap run chromium --shell", vec!["chromium"]),
            (
                "env A=1 nice -n5 ionice -tc2 -n4 setsid -fw nohup /opt/app",
                vec!["app"],
            ),
        ] {
            assert_eq!(exec_aliases(exec), aliases, "{exec}");
        }
    }

    #[test]
    fn launcher_invalid_or_ambiguous_forms_supply_no_aliases() {
        for exec in [
            "env -S 'operand app'",
            "env --split-string=app",
            "env -iSapp",
            "env -0 app",
            "env -u",
            "env -C",
            "env -a",
            "env -f",
            "env --ignore-signal= app",
            "env --ignore-environment=value app",
            "env A=1",
            "env =value app",
            "env -- A=1 -i app",
            "nice -n",
            "nice -napp /opt/app",
            "nice -x5 app",
            "nice --adjustment= app",
            "nice A=1 app",
            "nice --help app",
            "ionice -p123 app",
            "ionice -tP 123 app",
            "ionice --uid=123 app",
            "ionice -c",
            "ionice -n",
            "ionice -capp /opt/app",
            "ionice -n8 app",
            "dbus-run-session --dbus-daemon",
            "dbus-run-session --config-file",
            "dbus-run-session --help app",
            "dbus-launch --autolaunch=operand app",
            "dbus-launch --config-file operand app",
            "dbus-launch -- app",
            "setsid -fx app",
            "setsid --fork=true app",
            "setsid --help app",
            "nohup --help app",
            "nohup -x app",
            "systemd-run --unit",
            "systemd-run --property",
            "systemd-run -qS app",
            "systemd-run --system app",
            "systemd-run --expand-environment app",
            "systemd-run --unit= app",
            "taskset -p ff 123",
            "taskset -pc 123",
            "taskset -cp 0,1 123",
            "taskset -c",
            "taskset ff",
            "taskset --cpu-list=0,1 app",
            "taskset -c0,1 app",
            "taskset -c app /opt/app",
            "taskset -c 1-0 app",
            "taskset -c 0-3:0 app",
            "taskset -c 0,,1 app",
            "taskset mask app",
            "taskset -- ff",
            "taskset ff -x app",
            "flatpak run --branch",
            "flatpak run --arch",
            "flatpak run --command",
            "flatpak run --branch= org.example.App",
            "flatpak run --command= org.example.App",
            "flatpak run --command old --command '' org.example.App",
            "flatpak run --command old",
            "flatpak install run operand",
            "flatpak --arch=x86_64 run org.example.App",
            "flatpak run /opt/app",
            "flatpak run org.example.App/x86_64/stable/extra",
            "flatpak run org.example.App//stable",
            "flatpak run runtime/org.example.App/x86_64/stable",
            "flatpak run app/org.example.App/x86_64/stable",
            "flatpak run org.invalid-domain.App",
            "flatpak run org..App",
            "flatpak run %U",
            "snap run --shell music.player",
            "snap run --strace= music.player",
            "snap run --strace operand music.player",
            "snap run --gdbserver=localhost:1234 music.player",
            "snap run --command=old --command=new music.player",
            "snap run --hook=configure app",
            "snap run --revision=123 app",
            "snap run -r123 app",
            "snap run /opt/app",
            "snap run org.example.App",
            "snap run -invalid",
            "snap run -- -invalid",
            "snap run .app",
            "snap run app.",
            "snap list run operand",
            "snap run %U",
            "sh -c 'app'",
            "sh -ec 'exec app'",
            "sh -- app",
            "sh -c 'app' name argument",
            "bash -c app",
            "dash app",
            "zsh app",
            "ksh app",
            "fish app",
            "csh app",
            "tcsh app",
            "optirun app",
            "mangohud --dlsym app",
            "gamemoderun app",
            "primusrun app",
            "prime-run app",
            "steam-run app",
            "env %U",
            "env A=1 nice B=2 app",
            "env -ivx app",
        ] {
            assert!(exec_aliases(exec).is_empty(), "{exec}");
        }
        for spec in LAUNCHERS {
            let subcommand = if matches!(spec.operands, Operands::Flatpak | Operands::Snap) {
                "run "
            } else {
                ""
            };
            for options in ["--unknown-option operand", "--ver", "--unknown=operand"] {
                let exec = format!("{} {subcommand}{options} /opt/app", spec.name);
                assert!(exec_aliases(&exec).is_empty(), "{exec}");
            }
            assert!(exec_aliases(spec.name).is_empty(), "{}", spec.name);
        }
    }

    #[test]
    fn repeated_options_share_canonical_keys_and_keep_only_the_last_value() {
        // Inspect the shared parser as well as end-to-end aliases: no launcher
        // may retain earlier values under a different short/long spelling.
        for (launcher, args, key, expected) in [
            ("env", "-aold --argv0 new app", "argv0", Some("new")),
            (
                "env",
                "--ignore-signal=INT --ignore-signal app",
                "ignore-signal",
                None,
            ),
            (
                "nice",
                "-n5 --adjustment=6 --7 app",
                "adjustment",
                Some("-7"),
            ),
            ("ionice", "--class=1 -c2 app", "class", Some("2")),
            (
                "dbus-run-session",
                "--dbus-daemon old --dbus-daemon=new app",
                "dbus-daemon",
                Some("new"),
            ),
            (
                "dbus-launch",
                "--config-file=old --config-file=new app",
                "config-file",
                Some("new"),
            ),
            ("systemd-run", "-uold --unit=new app", "unit", Some("new")),
            (
                "flatpak",
                "--command=old --command new org.example.App",
                "command",
                Some("new"),
            ),
            ("setsid", "-ff --fork app", "fork", None),
            ("taskset", "-cc --cpu-list 0,1 app", "cpu-list", None),
            ("snap", "--debug-log --debug-log app", "debug-log", None),
        ] {
            let spec = LAUNCHERS.iter().find(|s| s.name == launcher).unwrap();
            let words = exec_words(args).unwrap();
            let parsed = launcher_args(spec, &words).unwrap();
            assert!(parsed.has(key), "{launcher} {args}");
            assert_eq!(parsed.value(key), expected, "{launcher} {args}");
            assert_eq!(parsed.values.len(), 1, "{launcher} {args}");
        }
        assert_eq!(exec_aliases("nice -ninvalid -n5 app"), vec!["app"]);
        assert!(exec_aliases("nice -n5 -ninvalid app").is_empty());
    }

    #[test]
    fn empty_quoted_launcher_operands_do_not_shift_the_executable_position() {
        assert_eq!(
            exec_words("env -a '' app argument").unwrap(),
            vec!["env", "-a", "", "app", "argument"]
        );
        for exec in [
            "env -a '' app argument",
            "env -u \"\" app argument",
            "dbus-run-session --dbus-daemon '' app argument",
            "optirun -b '' app argument",
            "flatpak run --command '' app argument",
            "snap run --command '' app argument",
            "flatpak run '' app argument",
            "'' app argument",
        ] {
            assert!(exec_aliases(exec).is_empty(), "{exec}");
        }
    }

    #[test]
    fn absolute_paths_also_match_desktop_ids_before_lower_priority_aliases() {
        let root = crate::history::test_dir();
        fixture(
            root.path(),
            "foo",
            "Name=Foo
Icon=foo-icon
Exec=/usr/bin/foo-launcher",
        );
        fixture(
            root.path(),
            "class",
            "Name=Class match
Icon=class-icon
StartupWMClass=/opt/foo",
        );
        let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], "C");
        expect(&index, "/opt/foo", "Foo", "foo-icon");
        expect(&index, "/opt/foo.desktop", "Foo", "foo-icon");
        // A raw path remains the fallback when neither it nor its basename matches.
        let who = "/opt/unregistered";
        assert_eq!(index.policy(who), (who.into(), GENERIC_ICON.into()));
    }

    #[test]
    fn matching_rule_priority_precedes_visibility_and_alias_ties_are_stable() {
        let root = crate::history::test_dir();
        for (id, fields) in [
            ("match", "Name=Exact\nIcon=exact\nHidden=true"),
            (
                "a-class",
                "Name=Class\nStartupWMClass=match\nIcon=class\nNoDisplay=true",
            ),
            ("b-exec", "Name=Exec\nExec=/bin/match\nIcon=exec"),
            ("c-name", "Name=match\nIcon=name"),
            ("d-icon", "Name=Icon\nIcon=match"),
        ] {
            fixture(root.path(), id, fields);
        }
        for (remove, name, icon) in [
            ("match", "Exact", "exact"),
            ("a-class", "Class", "class"),
            ("b-exec", "Exec", "exec"),
            ("c-name", "match", "name"),
            ("d-icon", "Icon", "match"),
        ] {
            let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], "C");
            expect(&index, "match", name, icon);
            std::fs::remove_file(root.path().join(format!("applications/{remove}.desktop")))
                .unwrap();
        }
        for (id, fields) in [
            ("a-hidden", "Name=Hidden\nExec=shared\nHidden=true"),
            ("b-unlisted", "Name=Unlisted\nExec=shared\nNoDisplay=true"),
            ("c-visible", "Name=Visible\nExec=shared\nIcon=visible"),
            ("d-visible", "Name=Other\nExec=shared\nIcon=other"),
        ] {
            fixture(root.path(), id, fields);
        }
        for _ in 0..3 {
            let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], "C");
            expect(&index, "shared", "Visible", "visible");
            expect(&index, "a-hidden", "Hidden", GENERIC_ICON);
            expect(&index, "b-unlisted", "Unlisted", GENERIC_ICON);
        }
    }
    #[test]
    fn xdg_override_replaces_all_aliases_including_hidden_entries() {
        let root = crate::history::test_dir();
        let user = root.path().join("user");
        let system = root.path().join("system");
        fixture(
            &user,
            "org.example.App",
            "Name=User\nExec=user-bin\nStartupWMClass=UserClass\nIcon=user\nHidden=true",
        );
        fixture(
            &system,
            "org.example.App",
            "Name=System\nExec=system-bin\nStartupWMClass=SystemClass\nIcon=system",
        );
        let index = DesktopIndex::from_dirs_for_locale(vec![user, system], "C");
        for who in ["org.example.App", "user-bin", "UserClass", "User", "user"] {
            expect(&index, who, "User", "user");
        }
        for who in ["system-bin", "SystemClass", "System", "system"] {
            assert_eq!(index.policy(who), (who.into(), GENERIC_ICON.into()));
        }
    }
    #[test]
    fn resolved_identity_flows_to_policies_history_and_return_windows() {
        use crate::{
            history::{Store, Tracker},
            model::{Blocker, Policy},
        };
        let root = crate::history::test_dir();
        fixture(
            root.path(),
            "com.anthropic.Claude",
            "Name=Claude\nExec=/usr/lib/claude-desktop/claude-desktop %U\nIcon=claude-desktop",
        );
        let index = DesktopIndex::from_dirs_for_locale(vec![root.path().into()], "C");
        let policy = Policy {
            what: "idle:sleep".into(),
            who: "claude-desktop".into(),
            reason: "Electron".into(),
            mode: "block".into(),
            flags: 1,
        };
        for (powerdevil, logind, source) in [
            (vec![policy.clone()], vec![], "powerdevil"),
            (vec![], vec![policy], "logind"),
        ] {
            let locks = crate::model::lock_blockers(&powerdevil, &logind, |who| index.policy(who));
            assert_eq!(locks.len(), 2);
            for lock in locks {
                assert_eq!(
                    (
                        lock.app_name.as_str(),
                        lock.icon_name.as_str(),
                        lock.source.as_str()
                    ),
                    ("Claude", "claude-desktop", source)
                );
            }
        }
        // The same resolve invocation as adapters::bridge, without desktopFileName.
        let (app_name, icon_name) =
            index.resolve("", "", "", "/usr/lib/claude-desktop/claude-desktop");
        let blocker = Blocker {
            internal_id: "window-1".into(),
            app_id: "claude-desktop".into(),
            app_name,
            icon_name,
            caption: "Conversation".into(),
            since: 100,
        };
        let mut tracker = Tracker::default();
        tracker.update(100, true, &[blocker]);
        let entries = tracker.update(800, false, &[]);
        let mut store = Store::open(root.path(), 800).unwrap();
        store.write(&entries, 800).unwrap();
        let history = store.history(7, 800).unwrap();
        let event = crate::notification::return_event(&entries).unwrap();
        for b in [&history[0].blocker, &event.windows[0].blocker] {
            assert_eq!(
                (b.app_name.as_str(), b.icon_name.as_str()),
                ("Claude", "claude-desktop")
            );
        }
    }
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
