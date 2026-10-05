// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    adapters,
    identity::DesktopIndex,
    model::{self, View},
};
use zbus::Connection;

#[derive(Default)]
pub struct Activation {
    away: bool,
    attempted: bool,
}
impl Activation {
    pub fn observe(&mut self, away: bool, available: bool) {
        let away = away && available;
        if !away {
            self.attempted = false;
        }
        self.away = away;
    }
    pub fn eligible(&self, view: &View) -> bool {
        !self.attempted && self.conditions(view)
    }
    pub fn conditions(&self, view: &View) -> bool {
        self.away
            && model::ignore_bypass_available(view, self.away)
            && view.running_since == 0
            && view.off_reason.is_empty()
            && !view.blockers.is_empty()
            && view.blockers.iter().all(|b| model::ignored(view, b))
    }
    /// Consume the away-period attempt immediately before calling Preview.
    pub fn dispatch(&mut self, view: &View) -> bool {
        if !self.eligible(view) {
            return false;
        }
        self.attempted = true;
        true
    }
    /// A non-cancellation failure also consumes the single allowed attempt.
    pub fn failed(&mut self) {
        self.attempted = true;
    }
}

pub type Validated = (
    String,
    Vec<model::Policy>,
    Result<adapters::BridgeSnapshot, adapters::BridgeFailure>,
);
#[derive(Debug)]
pub enum ValidationFailure {
    Policy(zbus::Error),
    Saver(zbus::Error),
}
pub type ValidationResult = Result<Result<Validated, ValidationFailure>, String>;

/// Capture the saver owner and showing state, then obtain a fresh exact snapshot.
/// No process is launched: Preview is sent only to this running unique owner.
pub async fn validate(
    conn: &Connection,
    index: &DesktopIndex,
    view: &View,
) -> Result<Validated, ValidationFailure> {
    let bus = adapters::proxy(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    .map_err(ValidationFailure::Saver)?;
    let owner: String = bus
        .call("GetNameOwner", &(adapters::SAVER_NAME,))
        .await
        .map_err(ValidationFailure::Saver)?;
    let requested = adapters::requested_policies(conn)
        .await
        .map_err(ValidationFailure::Policy)?;
    let bridge = adapters::bridge(conn, index, &view.blockers).await;
    let current: String = bus
        .call("GetNameOwner", &(adapters::SAVER_NAME,))
        .await
        .map_err(ValidationFailure::Saver)?;
    if current != owner {
        return Err(ValidationFailure::Saver(zbus::Error::Failure(
            "Screensaver owner changed".into(),
        )));
    }
    Ok((owner, requested, bridge))
}
pub async fn preview(
    conn: &Connection,
    owner: &str,
    data: &std::sync::Arc<std::sync::Mutex<crate::api::Data>>,
    generation: u64,
) -> zbus::Result<()> {
    prepared_preview(
        adapters::proxy(
            conn,
            owner,
            "/PlasmaVisualScreensaver",
            "org.kde.PlasmaVisualScreensaver",
        ),
        |saver| async move { saver.call("Preview", &()).await },
        data,
        generation,
    )
    .await
}
/// Keep proxy preparation and the final guard together, with no await between
/// consuming the attempt and polling the Preview call. The observation loop
/// records preparation errors/timeouts; dropping this future before dispatch
/// never consumes an attempt.
async fn prepared_preview<P, F, C, R>(
    prepare: F,
    call: C,
    data: &std::sync::Arc<std::sync::Mutex<crate::api::Data>>,
    generation: u64,
) -> zbus::Result<()>
where
    F: std::future::Future<Output = zbus::Result<P>>,
    C: FnOnce(P) -> R,
    R: std::future::Future<Output = zbus::Result<()>>,
{
    let saver = prepare.await?;
    {
        let mut d = data.lock().unwrap();
        let view = d.view.clone();
        if d.generation != generation || !d.activation.dispatch(&view) {
            return Ok(());
        }
    }
    call(saver).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> View {
        let mut v = model::view(&crate::config::Config {
            timeout: 60,
            conflicts: vec![],
        });
        v.exact = true;
        v.unavailable_code.clear();
        v.state = "ready".into();
        v.ignored_apps = vec!["ignored".into()];
        v.blockers.push(model::Blocker {
            internal_id: "1".into(),
            app_id: "ignored".into(),
            app_name: "App".into(),
            icon_name: String::new(),
            caption: String::new(),
            since: 1,
        });
        v
    }
    fn data() -> (
        tempfile::TempDir,
        std::sync::Arc<std::sync::Mutex<crate::api::Data>>,
    ) {
        let root = crate::paths::test_root();
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let mut data = crate::api::Data {
            view: view(),
            generation: 0,
            store: crate::history::Store::open(dir.path(), 100).unwrap(),
            tracker: Default::default(),
            preferences: Default::default(),
            activation: Default::default(),
            tracking_input: (true, true),
            notices: Default::default(),
        };
        data.activation.observe(true, true);
        (dir, std::sync::Arc::new(std::sync::Mutex::new(data)))
    }
    #[tokio::test]
    async fn cancelling_preparation_leaves_same_away_period_eligible() {
        let (_dir, data) = data();
        let calls = std::cell::Cell::new(0);
        {
            let prepare = prepared_preview(
                std::future::pending::<zbus::Result<()>>(),
                |()| async {
                    calls.set(calls.get() + 1);
                    Ok(())
                },
                &data,
                0,
            );
            tokio::pin!(prepare);
            // Poll preparation once, then drop it as the event guard does.
            assert!(futures_util::poll!(&mut prepare).is_pending());
        }
        let mut d = data.lock().unwrap();
        d.generation += 1;
        assert!(d.activation.eligible(&d.view));
        // A newly observed non-ignored blocker may come and leave without
        // resetting input tracking or consuming the cancelled attempt.
        let mut other = d.view.blockers[0].clone();
        other.app_id = "other".into();
        d.view.blockers.push(other);
        assert!(!d.activation.eligible(&d.view));
        d.view.blockers.pop();
        drop(d);
        prepared_preview(
            async { Ok(()) },
            |()| async {
                calls.set(calls.get() + 1);
                Ok(())
            },
            &data,
            1,
        )
        .await
        .unwrap();
        assert_eq!(calls.get(), 1);
        let d = data.lock().unwrap();
        assert!(!d.activation.eligible(&d.view));
    }
    #[tokio::test]
    async fn final_guard_skips_stale_or_ineligible_without_consuming_attempt() {
        for stale in [false, true] {
            let (_dir, data) = data();
            let calls = std::cell::Cell::new(0);
            let prepare = async {
                let mut d = data.lock().unwrap();
                if stale {
                    d.generation += 1;
                } else {
                    d.view.running_since = 1;
                }
                Ok(())
            };
            prepared_preview(
                prepare,
                |()| async {
                    calls.set(calls.get() + 1);
                    Ok(())
                },
                &data,
                0,
            )
            .await
            .unwrap();
            assert_eq!(calls.get(), 0);
            let mut d = data.lock().unwrap();
            d.view.running_since = 0;
            assert!(d.activation.eligible(&d.view));
        }
    }
    #[tokio::test]
    async fn preparation_error_or_timeout_does_not_dispatch() {
        for timeout in [false, true] {
            let (_dir, data) = data();
            let calls = std::cell::Cell::new(0);
            let prepare = async {
                if timeout {
                    std::future::pending::<()>().await;
                }
                Err::<(), _>(zbus::Error::Failure("proxy preparation failed".into()))
            };
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(1),
                prepared_preview(
                    prepare,
                    |()| async {
                        calls.set(calls.get() + 1);
                        Ok(())
                    },
                    &data,
                    0,
                ),
            )
            .await;
            if timeout {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().is_err());
            }
            assert_eq!(calls.get(), 0);
            // The observation-loop error handler records these failures.
            let d = data.lock().unwrap();
            assert!(d.activation.eligible(&d.view));
        }
    }
    #[tokio::test]
    async fn dispatch_success_error_timeout_and_cancellation_all_consume_attempt() {
        for outcome in ["success", "error", "timeout", "cancel"] {
            let (_dir, data) = data();
            let calls = std::cell::Cell::new(0);
            let preview = prepared_preview(
                async { Ok(()) },
                |()| async {
                    calls.set(calls.get() + 1);
                    match outcome {
                        "success" => Ok(()),
                        "error" => Err(zbus::Error::Failure("Preview failed".into())),
                        _ => std::future::pending().await,
                    }
                },
                &data,
                0,
            );
            if outcome == "cancel" {
                tokio::pin!(preview);
                assert!(futures_util::poll!(&mut preview).is_pending());
                // Drop after Preview has been polled: dispatch is uncertain.
            } else {
                let result =
                    tokio::time::timeout(std::time::Duration::from_millis(1), preview).await;
                match outcome {
                    "success" => result.unwrap().unwrap(),
                    "error" => assert!(result.unwrap().is_err()),
                    _ => assert!(result.is_err()),
                }
            }
            // Even an unchanged eligible view cannot dispatch twice.
            prepared_preview(
                async { Ok(()) },
                |()| async {
                    calls.set(calls.get() + 1);
                    Ok(())
                },
                &data,
                0,
            )
            .await
            .unwrap();
            assert_eq!(calls.get(), 1, "{outcome}");
            let mut d = data.lock().unwrap();
            assert!(!d.activation.eligible(&d.view), "{outcome}");
            d.activation.observe(false, true);
            d.activation.observe(true, true);
            assert!(d.activation.eligible(&d.view), "{outcome}");
        }
    }
    #[test]
    fn last_blocker_leaving_and_one_attempt_per_away_period() {
        let mut a = Activation::default();
        let mut v = view();
        assert!(!a.eligible(&v));
        a.observe(true, true);
        let mut other = v.blockers[0].clone();
        other.app_id = "other".into();
        v.blockers.push(other);
        assert!(!a.eligible(&v));
        assert_eq!(model::effective_blockers(&v).count(), 1);
        v.blockers.pop();
        assert!(a.dispatch(&v));
        assert!(!a.dispatch(&v));
        a.observe(true, true);
        assert!(!a.dispatch(&v));
        a.observe(false, true);
        a.observe(true, true);
        assert!(a.dispatch(&v));
    }
    #[test]
    fn requires_exact_available_identified_nonempty_and_not_showing() {
        let mut a = Activation::default();
        a.observe(true, false);
        assert!(!a.eligible(&view()));
        a.observe(true, true);
        for change in 0..7 {
            let mut v = view();
            match change {
                0 => v.exact = false,
                1 => v.unavailable_code = "unavailable".into(),
                2 => v.blockers.clear(),
                3 => v.blockers[0].app_id.clear(),
                4 => v.running_since = 1,
                5 => v.unattributed = true,
                _ => v.off_reason = "not running".into(),
            }
            assert!(!a.eligible(&v));
        }
        let v = view();
        assert_eq!(model::effective_blockers(&v).count(), 0);
        assert!(a.eligible(&v));
    }
}
