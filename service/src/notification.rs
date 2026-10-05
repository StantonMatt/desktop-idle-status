//! Pure return-event decisions and presentation helpers; the plasmoid owns delivery.
use crate::{
    history::{Entry, duration, union_seconds},
    model::is_unattributed,
};
use std::collections::HashMap;
#[derive(Debug, PartialEq, Eq)]
pub struct ReturnEvent {
    pub start: i64,
    pub end: i64,
    pub windows: Vec<ReturnWindow>,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ReturnWindow {
    pub blocker: crate::model::Blocker,
    pub seconds: u32,
}
/// Encode the union duration in end-start, even if the blocked time has gaps.
/// History remains the source for actual per-window timestamps.
pub fn return_event(entries: &[Entry]) -> Option<ReturnEvent> {
    let seconds = union_seconds(entries);
    if seconds < 600 {
        return None;
    }
    let end = entries.iter().map(|e| e.end).max()?;
    let mut per_window = HashMap::<&str, Vec<Entry>>::new();
    for entry in entries {
        per_window
            .entry(&entry.blocker.internal_id)
            .or_default()
            .push(entry.clone());
    }
    let mut ordered: Vec<_> = per_window
        .into_values()
        .map(|rows| {
            let first = rows
                .iter()
                .min_by_key(|e| e.start)
                .expect("nonempty window");
            let mut blocker = first.blocker.clone();
            blocker.since = first.start;
            if is_unattributed(&blocker) {
                blocker.app_name = "Unidentified window".into();
                blocker.icon_name = "preferences-system-windows".into();
                blocker.caption.clear();
            }
            // Saturate the wire's u32 duration instead of wrapping long intervals.
            ReturnWindow {
                blocker,
                seconds: u32::try_from(union_seconds(&rows)).unwrap_or(u32::MAX),
            }
        })
        .collect();
    ordered.sort_by(|a, b| {
        b.seconds.cmp(&a.seconds).then_with(|| {
            (
                a.blocker.since,
                &a.blocker.app_name,
                &a.blocker.caption,
                &a.blocker.internal_id,
            )
                .cmp(&(
                    b.blocker.since,
                    &b.blocker.app_name,
                    &b.blocker.caption,
                    &b.blocker.internal_id,
                ))
        })
    });
    let mut windows = Vec::new();
    for window in ordered {
        let b = &window.blocker;
        if !windows.iter().any(|w: &ReturnWindow| {
            let other = &w.blocker;
            (&other.app_name, &other.icon_name, &other.caption)
                == (&b.app_name, &b.icon_name, &b.caption)
        }) {
            windows.push(window);
        }
    }
    Some(ReturnEvent {
        start: end - seconds,
        end,
        windows,
    })
}
#[derive(Debug, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    pub icon: String,
}
pub fn content(seconds: i64, entries: &[Entry]) -> Option<Notice> {
    if seconds < 600 || entries.is_empty() {
        return None;
    }
    let mut apps = Vec::<String>::new();
    let mut per_app = HashMap::<String, Vec<Entry>>::new();
    for e in entries {
        let app = if is_unattributed(&e.blocker) {
            "an unidentified window".into()
        } else {
            e.blocker.app_name.clone()
        };
        if !apps.contains(&app) {
            apps.push(app.clone());
        }
        per_app.entry(app).or_default().push(e.clone());
    }
    let body = if entries.len() == 1 {
        let b = &entries[0].blocker;
        if is_unattributed(b) {
            "By an unidentified window".into()
        } else if b.caption.is_empty() {
            format!("By {}", b.app_name)
        } else {
            format!("By {} ({})", b.app_name, b.caption)
        }
    } else {
        match apps.len() {
            1 => format!("By {}", apps[0]),
            2 => format!("By {} and {}", apps[0], apps[1]),
            n => format!(
                "By {}, {} and {} {}",
                apps[0],
                apps[1],
                n - 2,
                if n == 3 { "other" } else { "others" }
            ),
        }
    };
    let longest = apps
        .iter()
        .filter_map(|app| per_app.get(app))
        .max_by_key(|rows| union_seconds(rows));
    let icon = longest
        .and_then(|rows| rows.first())
        .filter(|e| !is_unattributed(&e.blocker))
        .map(|e| e.blocker.icon_name.clone())
        .unwrap_or_else(|| "preferences-desktop-screensaver".into());
    Some(Notice {
        summary: format!(
            "Screensaver was blocked for {}",
            duration(seconds).replace(' ', "\u{a0}")
        ),
        body,
        icon,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Blocker;
    fn entry(id: &str, name: &str, seconds: i64) -> Entry {
        Entry {
            start: 0,
            end: seconds,
            blocker: Blocker {
                internal_id: id.into(),
                app_id: if id == "unattributed" {
                    String::new()
                } else {
                    name.into()
                },
                app_name: name.into(),
                icon_name: name.to_lowercase(),
                caption: "Movie".into(),
                since: 0,
            },
        }
    }
    #[test]
    fn return_event_uses_union_threshold_and_duration() {
        let a = entry("a", "Firefox", 599);
        assert!(
            return_event(&[a.clone(), a]).is_none(),
            "overlaps do not double duration"
        );
        let a = entry("a", "Firefox", 600);
        let event = return_event(&[a.clone(), a]).unwrap();
        assert_eq!((event.start, event.end, event.windows.len()), (0, 600, 1));
        assert_eq!(event.windows[0].seconds, 600);
        let mut b = entry("b", "Haruna", 1300);
        b.start = 1000;
        let event = return_event(&[entry("a", "Firefox", 300), b]).unwrap();
        assert_eq!(event.end - event.start, 600, "gaps do not add to duration");
        let event = return_event(&[entry("unattributed", "incorrect", 600)]).unwrap();
        assert_eq!(event.windows[0].blocker.app_name, "Unidentified window");
        assert_eq!(
            event.windows[0].blocker.icon_name,
            "preferences-system-windows"
        );
        assert!(event.windows[0].blocker.caption.is_empty());
    }
    #[test]
    fn return_windows_total_repeated_intervals_and_sort_longest_first() {
        let early = entry("early", "Firefox", 600);
        let mut late = entry("late", "Haruna", 900);
        late.start = 100;
        let mut repeated = entry("early", "Firefox", 1200);
        repeated.start = 900;
        let event = return_event(&[late, repeated, early]).unwrap();
        assert_eq!(event.end - event.start, 1200);
        assert_eq!(event.windows[0].blocker.internal_id, "early");
        assert_eq!(event.windows[0].seconds, 900, "released gaps do not count");
        assert_eq!(event.windows[1].seconds, 800);

        let mut overlap = entry("early", "Firefox", 1000);
        overlap.start = 300;
        let event = return_event(&[overlap, entry("early", "Firefox", 600)]).unwrap();
        assert_eq!(event.windows.len(), 1);
        assert_eq!(event.windows[0].seconds, 1000, "overlaps count once");
    }
    #[test]
    fn return_window_ties_use_earliest_start_then_display_order() {
        let early = entry("early", "Z", 600);
        let mut later = entry("later", "A", 700);
        later.start = 100;
        let tied = entry("tied", "B", 600);
        let event = return_event(&[later, early, tied]).unwrap();
        assert_eq!(
            event
                .windows
                .iter()
                .map(|w| w.blocker.internal_id.as_str())
                .collect::<Vec<_>>(),
            ["tied", "early", "later"]
        );
        assert!(event.windows.iter().all(|w| w.seconds == 600));
    }
    #[test]
    fn identical_display_rows_retain_longest_window_and_earliest_identity() {
        let early = entry("early", "Firefox", 600);
        let mut longer = entry("longer", "Firefox", 1000);
        longer.start = 100;
        let event = return_event(&[early, longer]).unwrap();
        assert_eq!(event.windows.len(), 1);
        assert_eq!(event.windows[0].blocker.internal_id, "longer");
        assert_eq!(event.windows[0].seconds, 900);

        let mut changed = entry("early", "Renamed", 1000);
        changed.start = 700;
        let event = return_event(&[changed, entry("early", "Firefox", 600)]).unwrap();
        assert_eq!(event.windows[0].blocker.app_name, "Firefox");
        assert_eq!(event.windows[0].seconds, 900);
    }
    #[test]
    fn return_seconds_saturate_to_wire_type() {
        let event = return_event(&[entry("a", "Firefox", i64::from(u32::MAX) + 1)]).unwrap();
        assert_eq!(event.windows[0].seconds, u32::MAX);
    }
    #[test]
    fn threshold_body_and_longest_icon() {
        let a = entry("a", "Firefox", 600);
        let b = entry("b", "Haruna", 900);
        assert!(content(599, std::slice::from_ref(&a)).is_none());
        assert_eq!(
            content(600, std::slice::from_ref(&a)).unwrap().body,
            "By Firefox (Movie)"
        );
        let c = content(900, &[a, b]).unwrap();
        assert_eq!(c.body, "By Firefox and Haruna");
        assert_eq!(c.icon, "haruna");
        assert_eq!(c.summary, "Screensaver was blocked for 15\u{a0}min");
        assert_eq!(
            content(600, &[entry("unattributed", "Unidentified window", 600)])
                .unwrap()
                .body,
            "By an unidentified window"
        );
    }
}
