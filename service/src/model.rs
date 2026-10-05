// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::config::Config;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocker {
    pub internal_id: String,
    pub app_id: String,
    pub app_name: String,
    pub icon_name: String,
    pub caption: String,
    pub since: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub what: String,
    pub who: String,
    pub reason: String,
    pub mode: String,
    pub flags: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockBlocker {
    pub app_name: String,
    pub icon_name: String,
    pub reason: String,
    pub what: String,
    pub source: String,
    pub mode: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairedProbe {
    pub normal_idle: bool,
    pub input_idle: bool,
}
impl PairedProbe {
    pub fn blocked(&self) -> Option<bool> {
        self.input_idle.then_some(!self.normal_idle)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub state: String,
    pub ignored_apps: Vec<String>,
    pub exact: bool,
    pub unavailable_code: String,
    pub unavailable_reason: String,
    pub timeout: u32,
    pub blockers: Vec<Blocker>,
    pub unattributed: bool,
    pub locks: Vec<LockBlocker>,
    pub conflicts: Vec<crate::config::Conflict>,
    pub running_since: i64,
    pub off_reason: String,
}
pub fn derive_state(
    running: bool,
    process: bool,
    exact: bool,
    blockers: bool,
    inferred: bool,
) -> &'static str {
    if running {
        "running"
    } else if !process {
        "screensaver-off"
    } else if blockers || (!exact && inferred) {
        "blocked"
    } else if exact {
        "ready"
    } else {
        "unknown"
    }
}
pub fn ignored(view: &View, blocker: &Blocker) -> bool {
    !blocker.app_id.is_empty() && view.ignored_apps.contains(&blocker.app_id)
}
pub fn effective_blockers(view: &View) -> impl Iterator<Item = &Blocker> {
    view.blockers.iter().filter(|b| !ignored(view, b))
}
/// Ignoring inhibitors promises a bypass only with exact attribution and a
/// working input-only tracker. Row preferences remain independent of this gate.
pub fn ignore_bypass_available(view: &View, input_available: bool) -> bool {
    input_available && view.exact && view.unavailable_code.is_empty() && !view.unattributed
}
pub fn has_blockers(view: &View, input_available: bool) -> bool {
    if ignore_bypass_available(view, input_available) {
        effective_blockers(view).next().is_some()
    } else {
        !view.blockers.is_empty()
    }
}
pub fn state_for_view(view: &View, input_available: bool) -> &'static str {
    derive_state(
        view.running_since != 0,
        view.off_reason.is_empty(),
        view.exact,
        has_blockers(view, input_available),
        view.unattributed,
    )
}
pub fn is_unattributed(blocker: &Blocker) -> bool {
    blocker.internal_id == "unattributed" && blocker.app_id.is_empty()
}
pub fn is_screensaver(identity: &str, reason: &str) -> bool {
    let name = identity
        .rsplit('/')
        .next()
        .unwrap_or(identity)
        .trim_end_matches(".desktop");
    matches!(
        name,
        "plasma-visual-screensaver"
            | "org.kde.plasmavisualscreensaver"
            | "org.kde.PlasmaVisualScreensaver"
    ) || (identity.is_empty() && reason == "Visual screensaver is active")
}
pub fn lock_blockers(
    policies: &[Policy],
    logind: &[Policy],
    resolve: impl Fn(&str) -> (String, String),
) -> Vec<LockBlocker> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (source, rows) in [("powerdevil", policies), ("logind", logind)] {
        for p in rows {
            if is_screensaver(&p.who, &p.reason)
                || p.mode != "block"
                || (source == "powerdevil" && p.flags & 1 == 0)
            {
                continue;
            }
            for what in p.what.split(':').filter(|w| matches!(*w, "idle" | "sleep")) {
                // Dedupe the imported logind tuple by original who/reason/what, not display name.
                if !seen.insert((
                    p.who.clone(),
                    p.reason.clone(),
                    what.to_owned(),
                    p.mode.clone(),
                )) {
                    continue;
                }
                let (app_name, icon_name) = resolve(&p.who);
                out.push(LockBlocker {
                    app_name,
                    icon_name,
                    reason: if p.reason.is_empty() {
                        "Unknown reason.".into()
                    } else {
                        p.reason.clone()
                    },
                    what: what.into(),
                    source: source.into(),
                    mode: p.mode.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.app_name, &a.what, &a.reason).cmp(&(&b.app_name, &b.what, &b.reason)));
    out
}
pub fn view(config: &Config) -> View {
    View {
        state: "unknown".into(),
        ignored_apps: vec![],
        exact: false,
        unavailable_code: "initializing".into(),
        unavailable_reason: "Initializing".into(),
        timeout: config.timeout,
        blockers: vec![],
        unattributed: false,
        locks: vec![],
        conflicts: config.conflicts.clone(),
        running_since: 0,
        off_reason: String::new(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ignored_and_unidentified_mixes_determine_state() {
        let mut v = view(&Config {
            timeout: 60,
            conflicts: vec![],
        });
        v.exact = true;
        v.unavailable_code.clear();
        v.ignored_apps = vec!["ignored".into()];
        let b = Blocker {
            internal_id: "one".into(),
            app_id: "ignored".into(),
            app_name: "App".into(),
            icon_name: String::new(),
            caption: String::new(),
            since: 1,
        };
        v.blockers.push(b.clone());
        assert_eq!(state_for_view(&v, true), "ready");
        assert_eq!(state_for_view(&v, false), "blocked");
        v.exact = false;
        assert_eq!(state_for_view(&v, true), "blocked");
        v.exact = true;
        v.unavailable_code = "stale".into();
        assert_eq!(state_for_view(&v, true), "blocked");
        v.unavailable_code.clear();
        for id in ["other", ""] {
            let mut other = b.clone();
            other.app_id = id.into();
            v.blockers.push(other);
            assert_eq!(state_for_view(&v, true), "blocked");
            v.blockers.pop();
        }
        assert_eq!(derive_state(false, true, false, false, true), "blocked");
    }
    #[test]
    fn paired_probes_need_input_evidence() {
        let mut probe = PairedProbe::default();
        assert_eq!(probe.blocked(), None);
        probe.input_idle = true;
        assert_eq!(probe.blocked(), Some(true));
        probe.normal_idle = true;
        assert_eq!(probe.blocked(), Some(false));
        probe.input_idle = false;
        assert_eq!(probe.blocked(), None);
    }
    #[test]
    fn state_precedence_and_missing_bridge_never_ready() {
        assert_eq!(derive_state(true, false, false, true, true), "running");
        assert_eq!(
            derive_state(false, false, true, false, false),
            "screensaver-off"
        );
        assert_eq!(derive_state(false, true, false, false, false), "unknown");
        assert_eq!(derive_state(false, true, false, false, true), "blocked");
        assert_eq!(derive_state(false, true, true, false, true), "ready");
    }
    #[test]
    fn delay_and_other_logind_modes_are_not_blockers() {
        let p = Policy {
            what: "idle:sleep".into(),
            who: "NetworkManager".into(),
            reason: "flush".into(),
            mode: "delay".into(),
            flags: 1,
        };
        let unknown = Policy {
            mode: "other".into(),
            ..p.clone()
        };
        assert!(
            lock_blockers(std::slice::from_ref(&p), &[p.clone(), unknown], |s| (
                s.into(),
                "icon".into()
            ))
            .is_empty()
        );
        let block = Policy {
            mode: "block".into(),
            ..p
        };
        assert_eq!(
            lock_blockers(&[], &[block], |s| (s.into(), "icon".into())).len(),
            2
        );
    }
    #[test]
    fn dedupes_imports_without_merging_other_apps() {
        let p = Policy {
            what: "idle:sleep".into(),
            who: "app".into(),
            reason: "movie".into(),
            mode: "block".into(),
            flags: 3,
        };
        let own = Policy {
            who: "org.kde.plasmavisualscreensaver".into(),
            ..p.clone()
        };
        let off = Policy {
            who: "suppressed".into(),
            flags: 2,
            ..p.clone()
        };
        let out = lock_blockers(&[p.clone(), own, off], &[p], |s| (s.into(), "icon".into()));
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|b| b.source == "powerdevil"));
    }
}
