// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
use desktop_idle_status::{
    adapters::{self, now},
    api::{self, Api, Data},
    config::{ConfigReader, Profile, source_dirs},
    history::{Store, Tracker},
    identity::DesktopIndex,
    model::{self, Policy},
    ownership,
    retry::Retry,
    wayland::{self, IdleEvent, ObservedEvent, Settings},
};
use futures_util::StreamExt;
use notify::{RecursiveMode, Watcher};
use std::{
    cell::Cell,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
use zbus::{Connection, MatchRule, MessageStream, message::Type};

async fn signal_stream(
    conn: &Connection,
    system: bool,
) -> zbus::Result<futures_util::stream::SelectAll<MessageStream>> {
    let mut streams = futures_util::stream::SelectAll::new();
    let owner = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .build();
    streams.push(MessageStream::for_match_rule(owner, conn, Some(128)).await?);
    let sources = if system {
        vec![
            (
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.DBus.Properties",
            ),
            (
                "org.freedesktop.systemd1",
                "/org/freedesktop/systemd1",
                "org.freedesktop.DBus.Properties",
            ),
        ]
    } else {
        vec![
            (
                "org.kde.Solid.PowerManagement",
                adapters::POLICY_PATH,
                "org.freedesktop.DBus.Properties",
            ),
            (
                adapters::POWER_IFACE,
                adapters::POWER_PATH,
                adapters::POWER_IFACE,
            ),
            ("org.kde.KWin", "/DesktopIdleStatus", adapters::BRIDGE_IFACE),
            (
                "org.kde.KWin",
                "/Plugins",
                "org.freedesktop.DBus.Properties",
            ),
            (
                adapters::SAVER_NAME,
                "/PlasmaVisualScreensaver",
                "org.freedesktop.DBus.Properties",
            ),
            (
                adapters::SAVER_NAME,
                "/PlasmaVisualScreensaver",
                "org.kde.PlasmaVisualScreensaver",
            ),
        ]
    };
    for (sender, path, interface) in sources {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(sender)?
            .path(path)?
            .interface(interface)?
            .build();
        streams.push(MessageStream::for_match_rule(rule, conn, Some(128)).await?);
    }
    Ok(streams)
}
fn relevant(message: &zbus::Message, system: bool) -> bool {
    let h = message.header();
    let iface = h.interface().map(|v| v.as_str());
    let member = h.member().map(|v| v.as_str());
    let path = h.path().map(|v| v.as_str());
    if iface == Some("org.freedesktop.DBus")
        && member == Some("NameOwnerChanged")
        && let Ok((name, _, _)) = message.body().deserialize::<(String, String, String)>()
    {
        return if system {
            matches!(
                name.as_str(),
                "org.freedesktop.login1" | "org.freedesktop.systemd1"
            )
        } else {
            matches!(
                name.as_str(),
                "org.kde.KWin" | "org.kde.Solid.PowerManagement" | adapters::SAVER_NAME
            )
        };
    }
    if system {
        matches!(
            path,
            Some("/org/freedesktop/login1" | "/org/freedesktop/systemd1")
        ) && iface == Some("org.freedesktop.DBus.Properties")
    } else {
        (path == Some(adapters::POLICY_PATH) && iface == Some("org.freedesktop.DBus.Properties"))
            || (path == Some(adapters::POWER_PATH)
                && iface == Some(adapters::POWER_IFACE)
                && matches!(member, Some("profileChanged" | "configurationReloaded")))
            || (path == Some("/DesktopIdleStatus") && iface == Some(adapters::BRIDGE_IFACE))
            || (path == Some("/Plugins") && iface == Some("org.freedesktop.DBus.Properties"))
            || (path == Some("/PlasmaVisualScreensaver")
                && matches!(
                    iface,
                    Some("org.freedesktop.DBus.Properties" | "org.kde.PlasmaVisualScreensaver")
                ))
    }
}
// Watch each source, or its closest existing ancestor if absent. Re-arm when
// directory creation is observed; never create configuration directories.
fn watch_config_sources(
    watcher: &mut impl Watcher,
    sources: &[PathBuf],
    watched: &mut std::collections::HashSet<PathBuf>,
) {
    for source in sources {
        let mut dir = source.as_path();
        while !dir.is_dir() {
            let Some(parent) = dir.parent() else {
                break;
            };
            dir = parent;
        }
        if !watched.contains(dir) {
            match watcher.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    watched.insert(dir.to_owned());
                }
                Err(e) => eprintln!(
                    "Configuration watcher unavailable for {}: {e}",
                    dir.display()
                ),
            }
        }
    }
}
/// The latest independent source observations. Project each change immediately;
/// no later source read may extend the old tracker state.
struct Sources<'a> {
    config: desktop_idle_status::config::Config,
    index: &'a DesktopIndex,
    requested: Vec<Policy>,
    active: Vec<Policy>,
    logind: Vec<Policy>,
    process: bool,
    policy_available: bool,
    settings_tx: Option<&'a mpsc::UnboundedSender<Settings>>,
    settings: Cell<Settings>,
}
impl Sources<'_> {
    fn bridge(
        &self,
        at: i64,
        result: Result<adapters::BridgeSnapshot, adapters::BridgeFailure>,
        idle: &mut IdleState,
        data: &mut Data,
    ) -> bool {
        let previous = data.view.clone();
        let exact = result.is_ok();
        if exact != data.view.exact {
            idle.inferred = false;
        }
        match result {
            Ok(snapshot) => {
                data.view.exact = true;
                data.view.unavailable_code.clear();
                data.view.unavailable_reason.clear();
                data.view.blockers = snapshot.blockers;
            }
            Err(e) => {
                data.view.exact = false;
                data.view.unavailable_code = e.code.into();
                data.view.unavailable_reason = e.reason;
                data.view.blockers.clear();
            }
        }
        let changed = self.apply(at, idle, data);
        changed || previous != data.view
    }
    fn apply(&self, at: i64, idle: &mut IdleState, data: &mut Data) -> bool {
        if data.view.timeout != self.config.timeout {
            idle.away = false;
            idle.inferred = false;
            data.tracker.reset(at);
        }
        let mut next = data.view.clone();
        let running = self.process && adapters::saver_showing(&self.requested);
        next.running_since = if running {
            if next.running_since != 0 {
                next.running_since
            } else {
                at
            }
        } else {
            0
        };
        next.timeout = self.config.timeout;
        next.conflicts = self.config.conflicts.clone();
        next.unattributed = !next.exact && idle.available && idle.inferred && !running;
        next.locks = model::lock_blockers(&self.active, &self.logind, |who| self.index.policy(who));
        next.off_reason = if self.process {
            String::new()
        } else {
            "Screensaver process is not running".into()
        };
        next.state = model::derive_state(
            running,
            self.process,
            next.exact,
            !next.blockers.is_empty(),
            next.unattributed,
        )
        .into();
        if next.exact {
            next.unavailable_code.clear();
            next.unavailable_reason.clear();
        }
        if self.process && !self.policy_available {
            next.state = "unknown".into();
            if next.exact {
                next.unavailable_code = "policyagent-unavailable".into();
                next.unavailable_reason =
                    "Screensaver activity unavailable: PowerDevil PolicyAgent not responding"
                        .into();
            }
        }
        let next_settings = Settings {
            timeout: next.timeout,
            exact: next.exact,
        };
        let settings = self.settings.get();
        if settings.timeout != next_settings.timeout || settings.exact != next_settings.exact {
            if let Some(tx) = self.settings_tx {
                let _ = tx.send(next_settings);
            }
            self.settings.set(next_settings);
        }
        let previous = data.view.clone();
        let storage = data.observe_view(next, at, idle.away, idle.available);
        data.publication_changed(&previous, &storage)
    }
}
#[derive(Default)]
struct IdleState {
    away: bool,
    inferred: bool,
    available: bool,
}
impl IdleState {
    fn handle(
        &mut self,
        observed: ObservedEvent,
        data: &mut Data,
        sources: &Sources<'_>,
    ) -> (bool, Option<api::ReservedReturn>) {
        let at = observed.at;
        match observed.event {
            IdleEvent::Away => {
                self.away = true;
            }
            IdleEvent::Resumed => {
                self.away = false;
                self.inferred = false;
                data.tracker.update(at, false, &[]);
                let changed = sources.apply(at, self, data);
                return (changed, data.take_return());
            }
            IdleEvent::Reset | IdleEvent::Unavailable(_) => {
                self.away = false;
                self.inferred = false;
                if let IdleEvent::Unavailable(e) = observed.event {
                    self.available = false;
                    eprintln!("{e}");
                }
                data.tracker.reset(at);
            }
            IdleEvent::Inference(blocked) => self.inferred = blocked,
            IdleEvent::Available => self.available = true,
        }
        (sources.apply(at, self, data), None)
    }
}
/// Keep the Wayland receiver live throughout every bounded source refresh.
async fn responsive<F: std::future::Future>(
    future: F,
    idle_rx: &mut mpsc::UnboundedReceiver<ObservedEvent>,
    idle: &mut IdleState,
    data: &Arc<Mutex<Data>>,
    changed: &mpsc::UnboundedSender<()>,
    session: Option<&Connection>,
    sources: &Sources<'_>,
) -> F::Output {
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            Some(event) = idle_rx.recv() => {
                let (written, notice) = idle.handle(event, &mut data.lock().unwrap(), sources);
                if written { let _ = changed.send(()); }
                if let Some(notice) = notice && let Some(session) = session {
                    let _ = adapters::bounded(api::publish_return(session, data, notice)).await;
                }
            }
            result = &mut future => return result,
        }
    }
}

#[derive(Default)]
struct ServiceState {
    data: Option<Arc<Mutex<Data>>>,
    // Keep the exporting connection alive through the final history publication.
    session: Option<Connection>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help") {
        println!(
            "desktop-idle-status [--smoke]\n--smoke: read-only 15 s snapshot, no owned D-Bus name."
        );
        return Ok(());
    }
    if args.iter().any(|a| a != "--smoke") {
        return Err("Unknown argument (see --help)".into());
    }
    let smoke = args.iter().any(|a| a == "--smoke");
    // Subscribe before any startup I/O. One outer select cancels every await in
    // observation, including work inside event handlers and signal publication.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut state = ServiceState::default();
    let mut idle_task = None;
    let mut ownership_task = None;
    let (lost_tx, mut lost_rx) = mpsc::unbounded_channel::<String>();
    // Keep the DB lock through cancellation and the final flush, including smoke.
    let mut database_lock = None;
    let mut active_profile = None;
    let result = tokio::select! {
        biased;
        _ = term.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
        _ = tokio::time::sleep(Duration::from_secs(15)), if smoke => Ok(()),
        Some(reason) = lost_rx.recv() => Err(reason.into()),
        result = observe(smoke, &mut state, &mut idle_task, &mut active_profile,
            &mut database_lock, &mut ownership_task, lost_tx) => result,
    };
    // Stop observation, then retry the same pending entries without extending
    // their completed timestamps. Keep the database lock through every attempt.
    let flush = if let Some(data) = state.data {
        let at = now();
        let (mut changed, mut flush) = {
            let mut d = data.lock().unwrap();
            let previous = d.view.clone();
            let storage = d.finish_intervals(at, true);
            (d.publication_changed(&previous, &storage), storage)
        };
        for _ in 0..3 {
            if flush.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut d = data.lock().unwrap();
            let previous = d.view.clone();
            flush = d.flush_pending(at);
            changed |= d.publication_changed(&previous, &flush);
        }
        if changed && let Some(session) = &state.session {
            // A disconnected/lost service may no longer have clients, but a
            // normal shutdown must publish committed history before releasing
            // its bus connection. Publication cannot delay exit indefinitely.
            if let result @ (Err(_) | Ok(Err(_))) = adapters::bounded(api::publish(session)).await {
                eprintln!("Final history publication failed: {result:?}");
            }
        }
        if smoke {
            print_snapshot(&data.lock().unwrap(), active_profile);
        }
        flush.map(|_| ())
    } else {
        Ok(())
    };
    if let Some(task) = idle_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = ownership_task {
        task.abort();
        let _ = task.await;
    }
    flush?;
    result
}

async fn observe(
    smoke: bool,
    state: &mut ServiceState,
    idle_task: &mut Option<tokio::task::JoinHandle<()>>,
    active_profile: &mut Option<Profile>,
    database_lock: &mut Option<std::fs::File>,
    ownership_task: &mut Option<tokio::task::JoinHandle<()>>,
    lost_tx: mpsc::UnboundedSender<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
        && desktop_idle_status::paths::environment()
            .absolute("XDG_RUNTIME_DIR")
            .is_none()
    {
        return Err(
            "An absolute XDG_RUNTIME_DIR or explicit DBUS_SESSION_BUS_ADDRESS is required".into(),
        );
    }
    let session = Connection::session().await?;
    let mut system: Option<Connection> = None;
    let mut session_signals = signal_stream(&session, false).await?;
    let mut system_signals = None;
    let (mut can_suspend, mut is_vm) = (true, false);
    let config_dir = desktop_idle_status::paths::environment()
        .home("XDG_CONFIG_HOME", ".config")
        .ok_or("An absolute XDG_CONFIG_HOME or HOME is required")?;
    let data_home = desktop_idle_status::paths::environment()
        .home("XDG_DATA_HOME", ".local/share")
        .ok_or("An absolute XDG_DATA_HOME or HOME is required")?;
    *database_lock = Some(desktop_idle_status::history::lock_database(&data_home)?);
    let mut profile = None;
    let config_sources = source_dirs(&config_dir);
    let mut config_reader = ConfigReader::desktop();
    let config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
    let index = Arc::new(DesktopIndex::load());
    let data = Arc::new(Mutex::new(Data {
        view: model::view(&config),
        generation: 0,
        store: Store::open(&data_home, now())?,
        tracker: Tracker::default(),
        tracking_input: (false, false),
        notices: api::ReturnNotices::default(),
    }));
    state.data = Some(data.clone());
    let (changed_tx, mut changed_rx) = mpsc::unbounded_channel();
    if !smoke {
        session
            .object_server()
            .at(
                api::PATH,
                api::ApiInterface(Api {
                    data: data.clone(),
                    index: index.clone(),
                    connection: session.clone(),
                    changed: changed_tx.clone(),
                }),
            )
            .await?;
        let loss_stream = ownership::loss_stream(&session).await?;
        ownership::request(&session).await?;
        state.session = Some(session.clone());
        *ownership_task = Some(tokio::spawn(async move {
            let _ = lost_tx.send(ownership::wait_for_loss(loss_stream).await);
        }));
    }
    let (fs_tx, mut fs_rx) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && matches!(
                event.kind,
                notify::EventKind::Create(_)
                    | notify::EventKind::Modify(_)
                    | notify::EventKind::Remove(_)
            )
            && event.paths.iter().any(|p| {
                p.file_name().is_some_and(|f| {
                    matches!(
                        f.to_str(),
                        Some(
                            "plasma-visual-screensaverrc"
                                | "powerdevilrc"
                                | "kscreenlockerrc"
                                | "kdeglobals"
                                | "system.kdeglobals"
                                | "kde5rc"
                        )
                    )
                }) || p.is_dir()
                    || !p.exists()
            })
        {
            let _ = fs_tx.send(());
        }
    })?;
    let mut watch_sources = config_sources.clone();
    if let Some(file) = config_reader.legacy_global() {
        watch_sources.push(file.parent().unwrap().into());
    }
    let mut watched = std::collections::HashSet::new();
    watch_config_sources(&mut watcher, &watch_sources, &mut watched);
    let (settings_tx, settings_rx) = mpsc::unbounded_channel();
    let (idle_tx, mut idle_rx) = mpsc::unbounded_channel();
    let settings = Settings {
        timeout: config.timeout,
        exact: false,
    };
    settings_tx.send(settings)?;
    *idle_task = Some(tokio::spawn(wayland::run(settings_rx, idle_tx)));
    let mut idle = IdleState::default();
    let mut sources = Sources {
        config,
        index: &index,
        requested: vec![],
        active: vec![],
        logind: vec![],
        process: false,
        policy_available: false,
        settings_tx: Some(&settings_tx),
        settings: Cell::new(settings),
    };
    let mut refresh_session = true;
    let mut refresh_system = true;
    let mut refresh_bridge = true;
    let mut policy_retry = Retry::default();
    let mut profile_retry = Retry::default();
    let mut saver_retry = Retry::default();
    let mut system_retry = Retry::default();
    let mut logind_retry = Retry::default();
    let mut suspend_retry = Retry::default();
    let mut vm_retry = Retry::default();
    let mut signals_retry = Retry::default();
    let mut maintenance = tokio::time::interval(Duration::from_secs(1));
    maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut bridge_retry = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(5),
        Duration::from_secs(5),
    );
    bridge_retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let previous = data.lock().unwrap().view.clone();
        let mut observed_change = false;
        macro_rules! apply {
            () => {
                observed_change |= sources.apply(now(), &mut idle, &mut data.lock().unwrap())
            };
        }
        if refresh_bridge {
            let (generation, blockers) = {
                let d = data.lock().unwrap();
                (d.generation, d.view.blockers.clone())
            };
            let result = responsive(
                adapters::bounded(adapters::bridge(&session, &index, &blockers)),
                &mut idle_rx,
                &mut idle,
                &data,
                &changed_tx,
                (!smoke).then_some(&session),
                &sources,
            )
            .await
            .unwrap_or_else(|reason| Err(adapters::BridgeFailure::error(reason)));
            let at = now();
            let mut d = data.lock().unwrap();
            // API validation may have invalidated this query while it awaited.
            // Wayland projections also advance the generation; retry on churn.
            refresh_bridge = d.generation != generation;
            if !refresh_bridge {
                observed_change |= sources.bridge(at, result, &mut idle, &mut d);
            }
        }
        let instant = std::time::Instant::now();
        if refresh_session || policy_retry.due(instant) {
            match responsive(
                adapters::bounded(adapters::requested_policies(&session)),
                &mut idle_rx,
                &mut idle,
                &data,
                &changed_tx,
                (!smoke).then_some(&session),
                &sources,
            )
            .await
            {
                Ok(Ok(r)) => {
                    sources.requested = r;
                    sources.policy_available = true;
                }
                other => {
                    sources.requested.clear();
                    sources.policy_available = false;
                    eprintln!("PolicyAgent unavailable: {other:?}");
                }
            }
            apply!();
            // Requested rows describe showing state. Apply that observation
            // before waiting for the independent active lock/sleep rows.
            match responsive(
                adapters::bounded(adapters::active_policies(&session)),
                &mut idle_rx,
                &mut idle,
                &data,
                &changed_tx,
                (!smoke).then_some(&session),
                &sources,
            )
            .await
            {
                Ok(Ok(rows)) => sources.active = rows,
                other => {
                    sources.active.clear();
                    sources.policy_available = false;
                    eprintln!("Active PolicyAgent inhibitions unavailable: {other:?}");
                }
            }
            policy_retry.complete(std::time::Instant::now(), sources.policy_available);
            apply!();
        }
        if refresh_session || profile_retry.due(instant) {
            profile = match responsive(
                adapters::bounded(adapters::current_profile(&session)),
                &mut idle_rx,
                &mut idle,
                &data,
                &changed_tx,
                (!smoke).then_some(&session),
                &sources,
            )
            .await
            {
                Ok(Ok(p)) => Some(p),
                other => {
                    eprintln!("Active PowerDevil profile unavailable: {other:?}");
                    None
                }
            };
            *active_profile = profile;
            profile_retry.complete(std::time::Instant::now(), profile.is_some());
            sources.config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
            apply!();
        }
        if refresh_session || saver_retry.due(instant) {
            let present = responsive(
                adapters::bounded(adapters::saver_present(&session)),
                &mut idle_rx,
                &mut idle,
                &data,
                &changed_tx,
                (!smoke).then_some(&session),
                &sources,
            )
            .await;
            let success = matches!(present, Ok(Ok(_)));
            sources.process = present.ok().and_then(Result::ok).unwrap_or(false);
            saver_retry.complete(std::time::Instant::now(), success);
            apply!();
        }
        refresh_session = false;
        if system.as_ref().is_some_and(Connection::is_closed) {
            system = None;
            system_signals = None;
            refresh_system = true;
            sources.logind.clear();
            (can_suspend, is_vm) = (true, false);
            sources.config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
            apply!();
        }
        if system.is_none() && (refresh_system || system_retry.due(instant)) {
            system = match responsive(
                adapters::bounded(Connection::system()),
                &mut idle_rx,
                &mut idle,
                &data,
                &changed_tx,
                (!smoke).then_some(&session),
                &sources,
            )
            .await
            {
                Ok(Ok(conn)) => Some(conn),
                other => {
                    eprintln!("System bus unavailable: {other:?}");
                    None
                }
            };
            system_retry.complete(std::time::Instant::now(), system.is_some());
            if system.is_some() {
                refresh_system = true;
            }
        }
        if let Some(c) = &system {
            if system_signals.is_none() && (refresh_system || signals_retry.due(instant)) {
                system_signals = responsive(
                    adapters::bounded(signal_stream(c, true)),
                    &mut idle_rx,
                    &mut idle,
                    &data,
                    &changed_tx,
                    (!smoke).then_some(&session),
                    &sources,
                )
                .await
                .ok()
                .and_then(Result::ok);
                signals_retry.complete(std::time::Instant::now(), system_signals.is_some());
            }
            if refresh_system || logind_retry.due(instant) {
                let result = responsive(
                    adapters::bounded(adapters::logind(c)),
                    &mut idle_rx,
                    &mut idle,
                    &data,
                    &changed_tx,
                    (!smoke).then_some(&session),
                    &sources,
                )
                .await;
                let success = matches!(result, Ok(Ok(_)));
                match result {
                    Ok(Ok(rows)) => sources.logind = rows,
                    other => {
                        sources.logind.clear();
                        eprintln!("logind unavailable: {other:?}");
                    }
                }
                logind_retry.complete(std::time::Instant::now(), success);
                apply!();
            }
            if refresh_system || suspend_retry.due(instant) {
                let result = responsive(
                    adapters::bounded(adapters::can_suspend(c)),
                    &mut idle_rx,
                    &mut idle,
                    &data,
                    &changed_tx,
                    (!smoke).then_some(&session),
                    &sources,
                )
                .await;
                let success = matches!(result, Ok(Ok(_)));
                can_suspend = match result {
                    Ok(Ok(value)) => value,
                    other => {
                        eprintln!("Suspend capability unavailable: {other:?}");
                        true
                    }
                };
                suspend_retry.complete(std::time::Instant::now(), success);
                sources.config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
                apply!();
            }
            if refresh_system || vm_retry.due(instant) {
                let result = responsive(
                    adapters::bounded(adapters::is_vm(c)),
                    &mut idle_rx,
                    &mut idle,
                    &data,
                    &changed_tx,
                    (!smoke).then_some(&session),
                    &sources,
                )
                .await;
                let success = matches!(result, Ok(Ok(_)));
                is_vm = match result {
                    Ok(Ok(value)) => value,
                    other => {
                        eprintln!("Virtualization capability unavailable: {other:?}");
                        false
                    }
                };
                vm_retry.complete(std::time::Instant::now(), success);
                sources.config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
                apply!();
            }
        } else {
            sources.logind.clear();
            (can_suspend, is_vm) = (true, false);
            sources.config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
            apply!();
        }
        refresh_system = false;
        let next = data.lock().unwrap().view.clone();
        if !smoke && (next != previous || observed_change) {
            api::publish(&session).await?;
        }
        tokio::select! {
            Some(())=changed_rx.recv()=>{refresh_bridge=true;if !smoke{api::publish(&session).await?;}},
            _=bridge_retry.tick()=>{refresh_bridge=true;},
            _=maintenance.tick()=>{if config_reader.retry_pending {sources.config=config_reader.read(&config_sources,can_suspend,is_vm,profile);if sources.apply(now(), &mut idle, &mut data.lock().unwrap()) {let _=changed_tx.send(());}}},
            Some(())=fs_rx.recv()=>{for dir in watched.drain(){let _=watcher.unwatch(&dir);}watch_config_sources(&mut watcher,&watch_sources,&mut watched);sources.config=config_reader.read(&config_sources,can_suspend,is_vm,profile);if sources.apply(now(), &mut idle, &mut data.lock().unwrap()) {let _=changed_tx.send(());}},
            Some(event)=idle_rx.recv()=>{
                let (changed, notice) = idle.handle(event, &mut data.lock().unwrap(), &sources);
                if !smoke && changed {api::publish(&session).await?;}
                if let Some(notice) = notice && !smoke {api::publish_return(&session, &data, notice).await?;}
            },
            message=session_signals.next()=>{
                match message {Some(Ok(message)) if relevant(&message,false)=>{
                    refresh_session=true;refresh_bridge=true;
                    if message.header().interface().is_some_and(|i|i.as_str()=="org.freedesktop.DBus")
                        && let Ok((name, _, new)) = message.body().deserialize::<(String,String,String)>() {
                        let at = now();
                        {
                        let mut d = data.lock().unwrap();
                        match name.as_str() {
                            "org.kde.KWin" => {
                                idle.inferred = false;
                                sources.bridge(at, Err(adapters::BridgeFailure::error("KWin bus owner changed; bridge resync pending")), &mut idle, &mut d);
                                if !new.is_empty() {let _=settings_tx.send(sources.settings.get());}
                            }
                            adapters::SAVER_NAME => {
                                sources.process = !new.is_empty();
                                sources.apply(at, &mut idle, &mut d);
                            }
                            "org.kde.Solid.PowerManagement" => {
                                sources.requested.clear();
                                sources.active.clear();
                                sources.policy_available = false;
                                profile = None;
                                *active_profile = None;
                                sources.config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
                                sources.apply(at, &mut idle, &mut d);
                            }
                            _ => {}
                        }
                        }
                        if !smoke { api::publish(&session).await?; }
                    } else if message.header().interface().is_some_and(|i| i.as_str()==adapters::POWER_IFACE) {
                        if message.header().member().is_some_and(|m| m.as_str()=="profileChanged") {
                            profile = message.body().deserialize::<String>().ok().as_deref().and_then(Profile::from_id);
                            *active_profile = profile;
                        }
                        sources.config = config_reader.read(&config_sources, can_suspend, is_vm, profile);
                        if sources.apply(now(), &mut idle, &mut data.lock().unwrap()) {let _=changed_tx.send(());}
                    }

                },None=>break,_=>{}}
            },
            message=async {match &mut system_signals {Some(s)=>s.next().await,None=>std::future::pending().await}}=>{
                match message {Some(Ok(message)) if relevant(&message,true)=>{
                    refresh_system=true;
                    if message.header().interface().is_some_and(|i| i.as_str()=="org.freedesktop.DBus")
                        && let Ok((name, _, _))=message.body().deserialize::<(String,String,String)>() {
                        if name=="org.freedesktop.login1" {sources.logind.clear();}
                        if name=="org.freedesktop.systemd1" {
                            (can_suspend, is_vm)=(true,false);
                            sources.config=config_reader.read(&config_sources,can_suspend,is_vm,profile);
                        }
                        if sources.apply(now(), &mut idle, &mut data.lock().unwrap()) {let _=changed_tx.send(());}
                    }
                },None=>{system_signals=None;signals_retry.complete(std::time::Instant::now(),false);},_=>{}}
            },
        }
    }
    Ok(())
}
fn print_snapshot(d: &Data, profile: Option<Profile>) {
    println!(
        "State={}\nExactAttribution={}\nUnavailableCode={}\nUnavailableReason={}\nScreensaverTimeout={}\nBlockers={:?}\nBlockedUnattributed={}\nLockSleepBlockers={:?}\nTimeoutConflicts={:?}\nActivePowerProfile={:?}",
        d.view.state,
        d.view.exact,
        d.view.unavailable_code,
        d.view.unavailable_reason,
        d.view.timeout,
        d.view.blockers,
        d.view.unattributed,
        d.view.locks,
        d.view.conflicts,
        profile
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn resumed_input_is_processed_during_stalled_system_refresh_at_observed_time() {
        let base = desktop_idle_status::paths::test_root();
        std::fs::create_dir_all(&base).unwrap();
        let dir = tempfile::tempdir_in(base).unwrap();
        let config = desktop_idle_status::config::Config {
            timeout: 60,
            conflicts: vec![],
        };
        let mut view = model::view(&config);
        view.state = "blocked".into();
        view.exact = true;
        view.blockers.push(model::Blocker {
            internal_id: "a".into(),
            app_id: "a".into(),
            app_name: "A".into(),
            icon_name: "a".into(),
            caption: "private".into(),
            since: 100,
        });
        let data = Arc::new(Mutex::new(Data {
            view,
            generation: 0,
            store: Store::open(dir.path(), 100).unwrap(),
            tracker: Tracker::default(),
            tracking_input: (false, false),
            notices: Default::default(),
        }));
        let index = DesktopIndex::default();
        let sources = Sources {
            config,
            index: &index,
            requested: vec![],
            active: vec![],
            logind: vec![],
            process: true,
            policy_available: true,
            settings_tx: None,
            settings: Cell::new(Settings {
                timeout: 60,
                exact: false,
            }),
        };
        let mut idle = IdleState {
            available: true,
            inferred: true,
            ..Default::default()
        };
        idle.handle(
            ObservedEvent {
                at: 100,
                event: IdleEvent::Away,
            },
            &mut data.lock().unwrap(),
            &sources,
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(ObservedEvent {
            at: 699,
            event: IdleEvent::Resumed,
        })
        .unwrap();
        let (changed, mut changes) = mpsc::unbounded_channel();
        let stalled = responsive(
            std::future::pending::<()>(),
            &mut rx,
            &mut idle,
            &data,
            &changed,
            None,
            &sources,
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), stalled)
                .await
                .is_err()
        );
        assert!(!idle.away);
        assert!(!idle.inferred);
        changes.try_recv().unwrap();
        let mut d = data.lock().unwrap();
        let rows = d.store.history(7, 700).unwrap();
        assert_eq!((rows[0].start, rows[0].end), (100, 699));
        assert!(
            d.take_return().is_none(),
            "599 seconds must not qualify after delayed processing"
        );
    }
    #[tokio::test]
    async fn source_observations_close_intervals_before_unrelated_stalled_reads() {
        for sibling in [
            "bridge-disappearance",
            "bridge-unavailable",
            "bridge-owner-change",
            "policy-unavailable",
            "active-policy-unavailable",
            "policy-showing",
            "screensaver-presence-loss",
            "profile-timeout",
            "config-timeout",
            "suspend-capability-timeout",
            "vm-capability-timeout",
            "logind",
            "system-bus-loss",
            "wayland-inference",
            "wayland-reset",
            "wayland-unavailable",
            "api-bridge-disappearance",
            "api-bridge-unavailable",
        ] {
            for duration in [59, 599] {
                let base = desktop_idle_status::paths::test_root();
                std::fs::create_dir_all(&base).unwrap();
                let dir = tempfile::tempdir_in(base).unwrap();
                let config = desktop_idle_status::config::Config {
                    timeout: 60,
                    conflicts: vec![],
                };
                let index = DesktopIndex::default();
                let mut sources = Sources {
                    config: config.clone(),
                    index: &index,
                    requested: vec![],
                    active: vec![],
                    logind: vec![],
                    process: true,
                    policy_available: true,
                    settings_tx: None,
                    settings: Cell::new(Settings {
                        timeout: 60,
                        exact: false,
                    }),
                };
                let mut view = model::view(&config);
                view.exact = sibling != "wayland-inference";
                view.blockers = if view.exact {
                    vec![model::Blocker {
                        internal_id: "a".into(),
                        app_id: "a".into(),
                        app_name: "A".into(),
                        icon_name: "a".into(),
                        caption: "private".into(),
                        since: 100,
                    }]
                } else {
                    vec![]
                };
                let data = Arc::new(Mutex::new(Data {
                    view,
                    generation: 0,
                    store: Store::open(dir.path(), 100).unwrap(),
                    tracker: Tracker::default(),
                    tracking_input: (false, false),
                    notices: Default::default(),
                }));
                let mut idle = IdleState {
                    away: true,
                    available: true,
                    inferred: true,
                };
                sources.apply(100, &mut idle, &mut data.lock().unwrap());
                let at = 100 + duration;
                {
                    let mut d = data.lock().unwrap();
                    match sibling {
                        "bridge-disappearance" => {
                            sources.bridge(
                                at,
                                Ok(adapters::BridgeSnapshot {
                                    revision: 2,
                                    blockers: vec![],
                                }),
                                &mut idle,
                                &mut d,
                            );
                        }
                        "bridge-unavailable" | "bridge-owner-change" => {
                            sources.bridge(
                                at,
                                Err(adapters::BridgeFailure::error("gone")),
                                &mut idle,
                                &mut d,
                            );
                        }
                        "api-bridge-disappearance" => {
                            d.observe_bridge(
                                Ok(adapters::BridgeSnapshot {
                                    revision: 2,
                                    blockers: vec![],
                                }),
                                at,
                            )
                            .unwrap();
                        }
                        "api-bridge-unavailable" => {
                            d.observe_bridge(Err(adapters::BridgeFailure::error("gone")), at)
                                .unwrap();
                        }
                        "policy-unavailable" | "active-policy-unavailable" => {
                            sources.policy_available = false;
                            sources.apply(at, &mut idle, &mut d);
                        }
                        "policy-showing" => {
                            sources.requested.push(Policy {
                                what: "idle".into(),
                                who: adapters::SAVER_NAME.into(),
                                reason: "Visual screensaver is active".into(),
                                mode: "block".into(),
                                flags: 1,
                            });
                            sources.apply(at, &mut idle, &mut d);
                        }
                        "screensaver-presence-loss" => {
                            sources.process = false;
                            sources.apply(at, &mut idle, &mut d);
                        }
                        "profile-timeout"
                        | "config-timeout"
                        | "suspend-capability-timeout"
                        | "vm-capability-timeout" => {
                            sources.config.timeout = 120;
                            sources.apply(at, &mut idle, &mut d);
                        }
                        "logind" | "system-bus-loss" => {
                            // Lock/sleep rows alone do not end a window interval.
                            sources.logind.push(Policy {
                                what: "sleep".into(),
                                who: "app".into(),
                                reason: "working".into(),
                                mode: "block".into(),
                                flags: 1,
                            });
                            sources.apply(at - 1, &mut idle, &mut d);
                            assert_eq!(d.view.locks.len(), 1);
                            if sibling == "system-bus-loss" {
                                sources.logind.clear();
                                sources.apply(at, &mut idle, &mut d);
                                assert!(d.view.locks.is_empty());
                            }
                            sources.bridge(
                                at,
                                Ok(adapters::BridgeSnapshot {
                                    revision: 2,
                                    blockers: vec![],
                                }),
                                &mut idle,
                                &mut d,
                            );
                        }
                        "wayland-inference" => {
                            idle.handle(
                                ObservedEvent {
                                    at,
                                    event: IdleEvent::Inference(false),
                                },
                                &mut d,
                                &sources,
                            );
                        }
                        "wayland-reset" => {
                            idle.handle(
                                ObservedEvent {
                                    at,
                                    event: IdleEvent::Reset,
                                },
                                &mut d,
                                &sources,
                            );
                        }
                        "wayland-unavailable" => {
                            idle.handle(
                                ObservedEvent {
                                    at,
                                    event: IdleEvent::Unavailable("gone".into()),
                                },
                                &mut d,
                                &sources,
                            );
                        }
                        _ => unreachable!(),
                    }
                }
                let (tx, mut rx) = mpsc::unbounded_channel();
                tx.send(ObservedEvent {
                    at: 1000,
                    event: IdleEvent::Resumed,
                })
                .unwrap();
                let (changed, _) = mpsc::unbounded_channel();
                // The unrelated source never completes. The observed resume is
                // processed while it is stalled, without real-clock thresholds.
                let stalled = responsive(
                    std::future::pending::<()>(),
                    &mut rx,
                    &mut idle,
                    &data,
                    &changed,
                    None,
                    &sources,
                );
                assert!(
                    tokio::time::timeout(Duration::from_millis(1), stalled)
                        .await
                        .is_err()
                );
                let mut d = data.lock().unwrap();
                let rows = d.store.history(7, 1000).unwrap();
                if duration == 59 {
                    assert!(rows.is_empty(), "{sibling}: short interval entered history");
                } else {
                    assert_eq!(rows.len(), 1, "{sibling}");
                    assert_eq!((rows[0].start, rows[0].end), (100, at), "{sibling}");
                }
                assert_eq!(
                    d.notices.register(),
                    Some(1),
                    "{sibling}: unexpected return notification"
                );
                // A notice would have been reserved by responsive on resumed.
                assert!(
                    d.notices.claim(1),
                    "{sibling}: stalled read inflated a notice past 599 seconds"
                );
                assert!(d.take_return().is_none(), "{sibling}");
            }
        }
    }
    #[tokio::test]
    async fn wayland_start_observations_are_applied_during_stalled_reads() {
        for event in [
            IdleEvent::Away,
            IdleEvent::Available,
            IdleEvent::Inference(true),
        ] {
            let base = desktop_idle_status::paths::test_root();
            std::fs::create_dir_all(&base).unwrap();
            let dir = tempfile::tempdir_in(base).unwrap();
            let config = desktop_idle_status::config::Config {
                timeout: 60,
                conflicts: vec![],
            };
            let index = DesktopIndex::default();
            let sources = Sources {
                config: config.clone(),
                index: &index,
                requested: vec![],
                active: vec![],
                logind: vec![],
                process: true,
                policy_available: true,
                settings_tx: None,
                settings: Cell::new(Settings {
                    timeout: 60,
                    exact: false,
                }),
            };
            let mut view = model::view(&config);
            view.exact = !matches!(event, IdleEvent::Inference(_));
            if view.exact {
                view.blockers.push(model::Blocker {
                    internal_id: "a".into(),
                    app_id: "a".into(),
                    app_name: "A".into(),
                    icon_name: "a".into(),
                    caption: "private".into(),
                    since: 100,
                });
            }
            let data = Arc::new(Mutex::new(Data {
                view,
                generation: 0,
                store: Store::open(dir.path(), 100).unwrap(),
                tracker: Tracker::default(),
                tracking_input: (false, false),
                notices: Default::default(),
            }));
            let mut idle = IdleState {
                away: !matches!(event, IdleEvent::Away),
                available: !matches!(event, IdleEvent::Available),
                inferred: false,
            };
            let (tx, mut rx) = mpsc::unbounded_channel();
            tx.send(ObservedEvent { at: 100, event }).unwrap();
            tx.send(ObservedEvent {
                at: 699,
                event: IdleEvent::Resumed,
            })
            .unwrap();
            let (changed, _) = mpsc::unbounded_channel();
            let stalled = responsive(
                std::future::pending::<()>(),
                &mut rx,
                &mut idle,
                &data,
                &changed,
                None,
                &sources,
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(1), stalled)
                    .await
                    .is_err()
            );
            let mut d = data.lock().unwrap();
            let rows = d.store.history(7, 1000).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!((rows[0].start, rows[0].end), (100, 699));
            assert_eq!(d.notices.register(), Some(1));
        }
    }
    #[test]
    fn timeout_settings_and_tracker_reset_precede_unrelated_reads() {
        let base = desktop_idle_status::paths::test_root();
        std::fs::create_dir_all(&base).unwrap();
        let dir = tempfile::tempdir_in(base).unwrap();
        let config = desktop_idle_status::config::Config {
            timeout: 60,
            conflicts: vec![],
        };
        let index = DesktopIndex::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut sources = Sources {
            config: config.clone(),
            index: &index,
            requested: vec![],
            active: vec![],
            logind: vec![],
            process: true,
            policy_available: true,
            settings_tx: Some(&tx),
            settings: Cell::new(Settings {
                timeout: 60,
                exact: true,
            }),
        };
        let mut view = model::view(&config);
        view.exact = true;
        view.blockers.push(model::Blocker {
            internal_id: "a".into(),
            app_id: "a".into(),
            app_name: "A".into(),
            icon_name: "a".into(),
            caption: "private".into(),
            since: 100,
        });
        let mut data = Data {
            view,
            generation: 0,
            store: Store::open(dir.path(), 100).unwrap(),
            tracker: Tracker::default(),
            tracking_input: (false, false),
            notices: Default::default(),
        };
        let mut idle = IdleState {
            away: true,
            available: true,
            inferred: false,
        };
        sources.apply(100, &mut idle, &mut data);
        sources.config.timeout = 120;
        sources.apply(159, &mut idle, &mut data);
        assert_eq!(rx.try_recv().unwrap().timeout, 120);
        assert!(!idle.away);
        data.finish_intervals(1000, false).unwrap();
        assert!(data.store.history(7, 1000).unwrap().is_empty());
        assert!(data.take_return().is_none());
    }
}
