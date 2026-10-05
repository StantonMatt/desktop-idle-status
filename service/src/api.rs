// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    adapters,
    history::{Store, Tracker},
    model::{Blocker, View},
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use zbus::{
    Connection, fdo,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Str},
};

pub const NAME: &str = "io.github.StantonMatt.DesktopIdleStatus";
pub const PATH: &str = "/io/github/StantonMatt/DesktopIdleStatus";
pub const IFACE: &str = "io.github.StantonMatt.DesktopIdleStatus1";
pub type Row = HashMap<String, OwnedValue>;
pub fn s(v: impl Into<String>) -> OwnedValue {
    OwnedValue::from(Str::from(v.into()))
}
pub fn blocker_row(b: &Blocker) -> Row {
    HashMap::from([
        ("internalId".into(), s(b.internal_id.clone())),
        ("appId".into(), s(b.app_id.clone())),
        ("appName".into(), s(b.app_name.clone())),
        ("iconName".into(), s(b.icon_name.clone())),
        ("caption".into(), s(b.caption.clone())),
        ("since".into(), OwnedValue::from(b.since)),
    ])
}
/// IDs are never reused within a service lifetime. Claimed IDs need no storage.
#[derive(Default)]
pub struct ReturnNotices {
    next: u32,
    pending: HashSet<u32>,
}
impl ReturnNotices {
    pub fn register(&mut self) -> Option<u32> {
        self.next = self.next.checked_add(1)?;
        self.pending.insert(self.next);
        Some(self.next)
    }
    pub fn claim(&mut self, id: u32) -> bool {
        self.pending.remove(&id)
    }
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}
pub struct Data {
    pub view: View,
    pub generation: u64,
    pub store: Store,
    pub tracker: Tracker,
    pub tracking_input: (bool, bool),
    pub notices: ReturnNotices,
}
pub struct Api {
    pub index: Arc<crate::identity::DesktopIndex>,
    pub data: Arc<Mutex<Data>>,
    pub connection: Connection,
    pub changed: mpsc::UnboundedSender<()>,
}
/// A notice is reserved while the tracker/store lock is still held. Publication
/// can await other I/O, but can never make a cleared ID claimable again.
pub struct ReservedReturn {
    id: u32,
    event: crate::notification::ReturnEvent,
}
impl Data {
    /// Every observation/flush publishes either a changed view or committed
    /// history (including retention-only writes). Failed writes leave pending
    /// intervals intact and must not suppress a changed view.
    pub fn publication_changed(&self, previous: &View, storage: &rusqlite::Result<bool>) -> bool {
        if let Err(e) = storage {
            eprintln!("History write failed; completed intervals remain pending: {e}");
        }
        previous != &self.view || matches!(storage, Ok(true))
    }
    pub fn reserve_return(&mut self, entries: &[crate::history::Entry]) -> Option<ReservedReturn> {
        let event = crate::notification::return_event(entries)?;
        let id = self.notices.register()?;
        Some(ReservedReturn { id, event })
    }
    pub fn take_return(&mut self) -> Option<ReservedReturn> {
        let (_, entries) = self.tracker.take_return_notice()?;
        self.reserve_return(&entries)
    }
    /// Tracker changes and their writes/retention happen under this same lock.
    pub fn finish_intervals(&mut self, now: i64, reset: bool) -> rusqlite::Result<bool> {
        if reset {
            self.tracker.reset(now);
        } else {
            self.tracker.update(now, false, &[]);
        };
        self.flush_pending(now)
    }
    pub fn flush_pending(&mut self, now: i64) -> rusqlite::Result<bool> {
        let changed = self.store.write(self.tracker.pending(), now)?;
        self.tracker.committed();
        Ok(changed)
    }
    pub fn set_view(&mut self, view: View) {
        self.generation = self.generation.wrapping_add(1);
        self.view = view;
    }
    /// Apply source observations at their timestamp before any unrelated await.
    pub fn observe_view(
        &mut self,
        view: View,
        at: i64,
        away: bool,
        available: bool,
    ) -> rusqlite::Result<bool> {
        self.tracking_input = (away, available);
        self.set_view(view);
        let blockers = if self.view.unattributed {
            vec![Blocker {
                internal_id: "unattributed".into(),
                app_id: String::new(),
                app_name: "Unidentified window".into(),
                icon_name: "preferences-system-windows".into(),
                caption: String::new(),
                since: at,
            }]
        } else {
            self.view.blockers.clone()
        };
        self.tracker.update(
            at,
            away && available && self.view.state == "blocked",
            &blockers,
        );
        self.flush_pending(at)
    }
    pub fn observe_bridge(
        &mut self,
        result: Result<adapters::BridgeSnapshot, adapters::BridgeFailure>,
        at: i64,
    ) -> rusqlite::Result<bool> {
        let mut view = self.view.clone();
        match result {
            Ok(snapshot) => {
                view.exact = true;
                view.blockers = snapshot.blockers;
                view.unattributed = false;
                if view.unavailable_code != "policyagent-unavailable" {
                    view.unavailable_code.clear();
                    view.unavailable_reason.clear();
                    view.state = crate::model::derive_state(
                        view.running_since != 0,
                        view.off_reason.is_empty(),
                        true,
                        !view.blockers.is_empty(),
                        false,
                    )
                    .into();
                }
            }
            Err(failure) => invalidate(&mut view, failure),
        }
        if view == self.view {
            return self.flush_pending(at);
        }
        let (away, available) = self.tracking_input;
        self.observe_view(view, at, away, available)
    }
    pub fn clear_history(&mut self) -> rusqlite::Result<()> {
        self.store.clear()?;
        self.tracker.clear();
        self.notices.clear();
        Ok(())
    }
}
impl Api {
    async fn validated_view(&self) -> View {
        for _ in 0..3 {
            let (generation, previous) = {
                let d = self.data.lock().unwrap();
                (d.generation, d.view.clone())
            };
            if !previous.exact {
                return previous;
            }
            let result = adapters::bounded(adapters::bridge(
                &self.connection,
                &self.index,
                &previous.blockers,
            ))
            .await
            .unwrap_or_else(|reason| Err(adapters::BridgeFailure::error(reason)));
            let mut d = self.data.lock().unwrap();
            // Never invalidate a newer observation with an older failed query,
            // including an unload/recovery ABA with an identical-looking view.
            if d.generation != generation {
                continue;
            }
            let previous = d.view.clone();
            let storage = d.observe_bridge(result, adapters::now());
            if d.publication_changed(&previous, &storage) {
                let _ = self.changed.send(());
            }
            return d.view.clone();
        }
        // Continuous churn: do not claim an unvalidated positive result.
        let mut view = self.data.lock().unwrap().view.clone();
        invalidate(
            &mut view,
            adapters::BridgeFailure::error("Observation changed during validation"),
        );
        view
    }
}
fn invalidate(view: &mut View, failure: adapters::BridgeFailure) {
    view.exact = false;
    view.unavailable_code = failure.code.into();
    view.unavailable_reason = failure.reason;
    view.blockers.clear();
    view.unattributed = false;
    if !matches!(view.state.as_str(), "running" | "screensaver-off") {
        view.state = "unknown".into();
    }
}
fn lock_rows(view: &View) -> Vec<Row> {
    view.locks
        .iter()
        .map(|b| {
            HashMap::from([
                ("appName".into(), s(b.app_name.clone())),
                ("iconName".into(), s(b.icon_name.clone())),
                ("reason".into(), s(b.reason.clone())),
                ("what".into(), s(b.what.clone())),
                ("source".into(), s(b.source.clone())),
                ("mode".into(), s(b.mode.clone())),
            ])
        })
        .collect()
}
fn conflict_rows(view: &View) -> Vec<Row> {
    view.conflicts
        .iter()
        .map(|c| {
            HashMap::from([
                ("setting".into(), s(c.setting.clone())),
                ("seconds".into(), c.seconds.into()),
                ("kcm".into(), s(c.kcm.clone())),
            ])
        })
        .collect()
}
fn view_rows(view: &View) -> Row {
    fn rows(value: Vec<Row>) -> OwnedValue {
        OwnedValue::try_from(zbus::zvariant::Value::from(value)).expect("serializable rows")
    }
    HashMap::from([
        ("State".into(), s(view.state.clone())),
        ("ExactAttribution".into(), view.exact.into()),
        ("UnavailableCode".into(), s(view.unavailable_code.clone())),
        (
            "UnavailableReason".into(),
            s(view.unavailable_reason.clone()),
        ),
        ("ScreensaverTimeout".into(), view.timeout.into()),
        (
            "Blockers".into(),
            rows(view.blockers.iter().map(blocker_row).collect()),
        ),
        ("BlockedUnattributed".into(), view.unattributed.into()),
        ("LockSleepBlockers".into(), rows(lock_rows(view))),
        ("TimeoutConflicts".into(), rows(conflict_rows(view))),
        ("RunningSince".into(), view.running_since.into()),
        ("ScreensaverOffReason".into(), s(view.off_reason.clone())),
    ])
}
/// zbus's generated GetAll independently awaits each property getter. Delegate
/// method dispatch/introspection to the macro, but capture properties once.
pub struct ApiInterface(pub Api);
#[async_trait::async_trait]
impl zbus::object_server::Interface for ApiInterface {
    fn name() -> zbus::names::InterfaceName<'static> {
        Api::name()
    }
    async fn get(
        &self,
        property: &str,
        _server: &zbus::ObjectServer,
        _connection: &Connection,
        _header: Option<&zbus::message::Header<'_>>,
        _emitter: &SignalEmitter<'_>,
    ) -> Option<fdo::Result<OwnedValue>> {
        view_rows(&self.0.validated_view().await)
            .remove(property)
            .map(Ok)
    }
    async fn get_all(
        &self,
        _server: &zbus::ObjectServer,
        _connection: &Connection,
        _header: Option<&zbus::message::Header<'_>>,
        _emitter: &SignalEmitter<'_>,
    ) -> fdo::Result<Row> {
        Ok(view_rows(&self.0.validated_view().await))
    }
    async fn set_mut(
        &mut self,
        property: &str,
        value: &zbus::zvariant::Value<'_>,
        server: &zbus::ObjectServer,
        connection: &Connection,
        header: Option<&zbus::message::Header<'_>>,
        emitter: &SignalEmitter<'_>,
    ) -> Option<fdo::Result<()>> {
        self.0
            .set_mut(property, value, server, connection, header, emitter)
            .await
    }
    fn call<'a>(
        &'a self,
        server: &'a zbus::ObjectServer,
        connection: &'a Connection,
        message: &'a zbus::message::Message,
        name: zbus::names::MemberName<'a>,
    ) -> zbus::object_server::DispatchResult2<'a> {
        self.0.call(server, connection, message, name)
    }
    fn call_mut<'a>(
        &'a mut self,
        server: &'a zbus::ObjectServer,
        connection: &'a Connection,
        message: &'a zbus::message::Message,
        name: zbus::names::MemberName<'a>,
    ) -> zbus::object_server::DispatchResult2<'a> {
        self.0.call_mut(server, connection, message, name)
    }
    fn introspect_to_writer(&self, writer: &mut dyn std::fmt::Write, level: usize) {
        self.0.introspect_to_writer(writer, level);
    }
}
#[zbus::interface(name = "io.github.StantonMatt.DesktopIdleStatus1")]
impl Api {
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn state(&self) -> String {
        self.data.lock().unwrap().view.state.clone()
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn exact_attribution(&self) -> bool {
        self.data.lock().unwrap().view.exact
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn unavailable_code(&self) -> String {
        self.data.lock().unwrap().view.unavailable_code.clone()
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn unavailable_reason(&self) -> String {
        self.data.lock().unwrap().view.unavailable_reason.clone()
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn screensaver_timeout(&self) -> u32 {
        self.data.lock().unwrap().view.timeout
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn blockers(&self) -> Vec<Row> {
        self.data
            .lock()
            .unwrap()
            .view
            .blockers
            .iter()
            .map(blocker_row)
            .collect()
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn blocked_unattributed(&self) -> bool {
        self.data.lock().unwrap().view.unattributed
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn lock_sleep_blockers(&self) -> Vec<Row> {
        lock_rows(&self.data.lock().unwrap().view)
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn timeout_conflicts(&self) -> Vec<Row> {
        conflict_rows(&self.data.lock().unwrap().view)
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn running_since(&self) -> i64 {
        self.data.lock().unwrap().view.running_since
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn screensaver_off_reason(&self) -> String {
        self.data.lock().unwrap().view.off_reason.clone()
    }
    fn history(&self, days: u32) -> fdo::Result<Vec<Row>> {
        self.data
            .lock()
            .unwrap()
            .store
            .history(days, adapters::now())
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|e| {
                        let mut row = blocker_row(&e.blocker);
                        row.remove("since");
                        row.insert("start".into(), OwnedValue::from(e.start));
                        row.insert("end".into(), OwnedValue::from(e.end));
                        row
                    })
                    .collect()
            })
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }
    fn clear_history(&self) -> fdo::Result<()> {
        let mut data = self.data.lock().unwrap();
        data.clear_history()
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let _ = self.changed.send(());
        Ok(())
    }
    async fn activate_window(&self, internal_id: &str) -> bool {
        if !self.data.lock().unwrap().view.exact {
            return false;
        }
        adapters::bounded(adapters::activate_window(&self.connection, internal_id))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    }
    fn claim_return_notice(&self, id: u32) -> bool {
        self.data.lock().unwrap().notices.claim(id)
    }
    async fn start_screensaver(&self) -> bool {
        let Ok(mut child) = tokio::process::Command::new("plasma-visual-screensaver")
            .arg("--background")
            .spawn()
        else {
            return false;
        };
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        true
    }
    #[zbus(signal)]
    pub async fn blocked_while_away(
        emitter: &SignalEmitter<'_>,
        id: u32,
        start: i64,
        end: i64,
        windows: Vec<Row>,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    pub async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}
pub async fn publish(conn: &Connection) -> zbus::Result<()> {
    let invalidated = vec![
        "State",
        "ExactAttribution",
        "UnavailableCode",
        "UnavailableReason",
        "ScreensaverTimeout",
        "Blockers",
        "BlockedUnattributed",
        "LockSleepBlockers",
        "TimeoutConflicts",
        "RunningSince",
        "ScreensaverOffReason",
    ];
    conn.emit_signal(
        None::<&str>,
        PATH,
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
        &(IFACE, HashMap::<&str, OwnedValue>::new(), invalidated),
    )
    .await?;
    let emitter = SignalEmitter::new(conn, PATH)?;
    Api::changed(&emitter).await
}

/// Publish once on resumed input. Threshold and overlap decisions stay pure.
pub async fn publish_return(
    conn: &Connection,
    data: &Arc<Mutex<Data>>,
    notice: ReservedReturn,
) -> zbus::Result<()> {
    let ReservedReturn { id, event } = notice;
    if !data.lock().unwrap().notices.pending.contains(&id) {
        return Ok(());
    }
    let windows = event
        .windows
        .iter()
        .map(|window| {
            let b = &window.blocker;
            HashMap::from([
                ("seconds".into(), OwnedValue::from(window.seconds)),
                ("appName".into(), s(b.app_name.clone())),
                ("iconName".into(), s(b.icon_name.clone())),
                ("caption".into(), s(b.caption.clone())),
            ])
        })
        .collect();
    let emitter = SignalEmitter::new(conn, PATH)?;
    Api::blocked_while_away(&emitter, id, event.start, event.end, windows).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_intervals_survive_write_failures_for_update_reset_and_shutdown() {
        for reset in [false, true] {
            let dir = crate::history::test_dir();
            let mut d = Data {
                view: crate::model::view(&crate::config::Config {
                    timeout: 60,
                    conflicts: vec![],
                }),
                generation: 0,
                store: Store::open(dir.path(), 100).unwrap(),
                tracker: Tracker::default(),
                tracking_input: (false, false),
                notices: ReturnNotices::default(),
            };
            let db =
                rusqlite::Connection::open(dir.path().join("desktop-idle-status/history.sqlite3"))
                    .unwrap();
            db.execute_batch("CREATE TRIGGER reject BEFORE INSERT ON history BEGIN SELECT RAISE(FAIL,'injected write failure'); END;").unwrap();
            let blocker = Blocker {
                internal_id: "a".into(),
                app_id: "a".into(),
                app_name: "A".into(),
                icon_name: "a".into(),
                caption: "private".into(),
                since: 100,
            };
            d.tracker.update(100, true, &[blocker]);
            assert!(d.finish_intervals(700, reset).is_err());
            assert_eq!(d.tracker.pending().len(), 1);
            // Shutdown/reset must not throw away the already completed interval.
            assert!(d.finish_intervals(800, true).is_err());
            assert_eq!(d.tracker.pending()[0].end, 700);
            db.execute_batch("DROP TRIGGER reject;").unwrap();
            assert!(d.flush_pending(900).unwrap());
            assert!(d.tracker.pending().is_empty());
            assert!(!d.flush_pending(900).unwrap());
            assert_eq!(d.store.history(7, 900).unwrap().len(), 1);
        }
    }
    #[test]
    fn clear_cancels_uncommitted_intervals_and_reserved_notices() {
        let dir = crate::history::test_dir();
        let mut d = Data {
            view: crate::model::view(&crate::config::Config {
                timeout: 60,
                conflicts: vec![],
            }),
            generation: 0,
            store: Store::open(dir.path(), 100).unwrap(),
            tracker: Tracker::default(),
            tracking_input: (false, false),
            notices: ReturnNotices::default(),
        };
        let blocker = Blocker {
            internal_id: "a".into(),
            app_id: "a".into(),
            app_name: "A".into(),
            icon_name: "a".into(),
            caption: "private".into(),
            since: 100,
        };
        d.tracker.update(100, true, &[blocker]);
        d.tracker.update(700, false, &[]);
        let notice = d.take_return().unwrap();
        assert_eq!(d.tracker.pending().len(), 1);
        d.clear_history().unwrap();
        assert!(d.tracker.pending().is_empty());
        assert!(!d.notices.claim(notice.id));
        assert!(!d.flush_pending(800).unwrap());
        assert!(d.store.history(7, 800).unwrap().is_empty());
    }
    #[tokio::test]
    async fn clear_serializes_closed_intervals_reset_and_retention() {
        for reset in [false, true] {
            let dir = crate::history::test_dir();
            let config = crate::config::Config {
                timeout: 60,
                conflicts: vec![],
            };
            let data = Arc::new(Mutex::new(Data {
                view: crate::model::view(&config),
                generation: 0,
                store: Store::open(dir.path(), 100).unwrap(),
                tracker: Tracker::default(),
                tracking_input: (false, false),
                notices: ReturnNotices::default(),
            }));
            let blocker = Blocker {
                internal_id: "old".into(),
                app_id: "App".into(),
                app_name: "App".into(),
                icon_name: "icon".into(),
                caption: "private".into(),
                since: 100,
            };
            let reserved = {
                let mut d = data.lock().unwrap();
                d.tracker.update(100, true, std::slice::from_ref(&blocker));
                assert!(d.finish_intervals(700, reset).unwrap());
                assert_eq!(d.store.history(7, 700).unwrap().len(), 1);
                d.take_return()
            };
            let clearing = data.clone();
            // Clear while Changed/publication could be awaiting outside the lock.
            tokio::spawn(async move {
                clearing.lock().unwrap().clear_history().unwrap();
            })
            .await
            .unwrap();
            let mut d = data.lock().unwrap();
            assert!(!d.finish_intervals(800, reset).unwrap());
            if let Some(reserved) = reserved {
                assert!(!d.notices.claim(reserved.id));
            }
            // Both retention siblings run under the same lock and cannot restore
            // entries or revive cleared tracking/notice state.
            assert!(!d.store.trim(800 + crate::history::RETENTION).unwrap());
            assert!(!d.store.write(&[], 800 + crate::history::RETENTION).unwrap());
            assert!(d.store.history(7, 800).unwrap().is_empty());
            d.tracker.update(900, true, std::slice::from_ref(&blocker));
            d.clear_history().unwrap();
            assert!(!d.finish_intervals(1500, reset).unwrap());
            // The same still-present window may start a fresh post-clear interval.
            d.tracker.update(1600, true, &[blocker]);
            d.finish_intervals(1700, reset).unwrap();
            assert_eq!(d.store.history(7, 1700).unwrap()[0].start, 1600);
        }
    }
    #[test]
    fn notices_are_claimed_once_and_clear_does_not_reuse_ids() {
        let mut notices = ReturnNotices::default();
        assert!(!notices.claim(0));
        let first = notices.register().unwrap();
        assert!(!notices.claim(first + 1));
        assert!(notices.claim(first));
        assert!(!notices.claim(first));
        let cleared = notices.register().unwrap();
        notices.clear();
        assert!(!notices.claim(cleared));
        assert!(notices.register().unwrap() > cleared);
        notices.next = u32::MAX;
        assert!(notices.register().is_none(), "IDs must never wrap");
    }
}
