// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
//! Input-only away tracking plus fresh paired fallback probes. Never creates inhibitors.
use std::time::Duration;
use tokio::{
    io::unix::AsyncFd,
    sync::mpsc,
    time::{Instant, sleep_until},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{wl_callback, wl_registry, wl_seat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

#[derive(Debug)]
pub enum IdleEvent {
    Away,
    Resumed,
    Reset,
    Inference(bool),
    Available,
    Unavailable(String),
}
/// Unix timestamp captured in the Wayland dispatch callback, before source I/O.
#[derive(Debug)]
pub struct ObservedEvent {
    pub at: i64,
    pub event: IdleEvent,
}
impl From<IdleEvent> for ObservedEvent {
    fn from(event: IdleEvent) -> Self {
        Self {
            at: crate::adapters::now(),
            event,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub timeout: u32,
    pub exact: bool,
}
#[derive(Clone, Copy)]
enum Kind {
    Away,
    Normal,
    Input,
    Activity,
}
#[derive(Default)]
struct State {
    notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    seat: Option<wl_seat::WlSeat>,
    notifier_global: Option<u32>,
    seat_global: Option<u32>,
    initial_done: bool,
    disconnected: bool,
    probe: crate::model::PairedProbe,
    events: Vec<ObservedEvent>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "ext_idle_notifier_v1" => {
                if version >= 2 {
                    state.notifier = Some(registry.bind(name, 2, qh, ()));
                    state.notifier_global = Some(name);
                }
            }
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_seat" && state.seat.is_none() => {
                state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                state.seat_global = Some(name);
            }
            wl_registry::Event::GlobalRemove { name }
                if Some(name) == state.notifier_global || Some(name) == state.seat_global =>
            {
                state.disconnected = true
            }
            _ => {}
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ext_idle_notifier_v1::ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ext_idle_notifier_v1::ExtIdleNotifierV1,
        _: ext_idle_notifier_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.initial_done = true;
    }
}
impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, Kind> for State {
    fn event(
        state: &mut Self,
        _: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        kind: &Kind,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let idle = matches!(event, ext_idle_notification_v1::Event::Idled);
        match kind {
            Kind::Away => state.events.push(
                (if idle {
                    IdleEvent::Away
                } else {
                    IdleEvent::Resumed
                })
                .into(),
            ),
            Kind::Normal => state.probe.normal_idle = idle,
            Kind::Activity => {
                if !idle {
                    state.events.push(IdleEvent::Resumed.into());
                }
            }
            Kind::Input => {
                state.probe.input_idle = idle;
                if !idle {
                    state.events.push(IdleEvent::Inference(false).into());
                }
            }
        }
    }
}
pub async fn run(
    mut settings: mpsc::UnboundedReceiver<Settings>,
    events: mpsc::UnboundedSender<ObservedEvent>,
) {
    while let Some(initial) = settings.recv().await {
        match run_inner(&mut settings, &events, initial).await {
            Ok(()) => return,
            Err(e) => {
                let _ = events.send(IdleEvent::Unavailable(e).into());
            }
        }
        // Retry only on a settings/KWin lifecycle event; no reconnect busy loop.
    }
}
async fn run_inner(
    settings: &mut mpsc::UnboundedReceiver<Settings>,
    events: &mpsc::UnboundedSender<ObservedEvent>,
    initial: Settings,
) -> Result<(), String> {
    // libwayland permits absolute displays and inherited WAYLAND_SOCKET fds.
    // Relative display names require a valid XDG runtime directory.
    if std::env::var_os("WAYLAND_SOCKET").is_none()
        && !std::env::var_os("WAYLAND_DISPLAY")
            .is_some_and(|v| std::path::Path::new(&v).is_absolute())
        && crate::paths::environment()
            .absolute("XDG_RUNTIME_DIR")
            .is_none()
    {
        return Err(
            "Wayland connection unavailable: XDG_RUNTIME_DIR must be absolute and nonempty".into(),
        );
    }
    let conn =
        Connection::connect_to_env().map_err(|e| format!("Wayland connection unavailable: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    conn.display().sync(&qh, ());
    let backend = conn.backend();
    let fd = AsyncFd::new(backend.poll_fd()).map_err(|e| e.to_string())?;
    let mut state = State::default();
    let mut current = initial;
    let mut away: Option<ext_idle_notification_v1::ExtIdleNotificationV1> = None;
    let mut activity: Option<ext_idle_notification_v1::ExtIdleNotificationV1> = None;
    let mut pair: Vec<ext_idle_notification_v1::ExtIdleNotificationV1> = vec![];
    let mut refresh = Instant::now();
    let mut classify = Instant::now() + Duration::from_secs(86400);
    let initialization = Instant::now() + Duration::from_secs(5);
    loop {
        queue
            .dispatch_pending(&mut state)
            .map_err(|e| e.to_string())?;
        if state.disconnected {
            return Err("Wayland idle globals disappeared".into());
        }
        if state.initial_done && away.is_none() {
            let (Some(notifier), Some(seat)) = (&state.notifier, &state.seat) else {
                return Err("ext-idle-notify v2 or wl_seat unavailable".into());
            };
            away = Some(notifier.get_input_idle_notification(
                current.timeout.saturating_mul(1000),
                seat,
                &qh,
                Kind::Away,
            ));
            if activity.is_none() {
                activity =
                    Some(notifier.get_input_idle_notification(10_000, seat, &qh, Kind::Activity));
            }
            let _ = events.send(IdleEvent::Available.into());
        }
        for event in state.events.drain(..) {
            let _ = events.send(event);
        }
        // Classification is deferred briefly to let both notifications in a round arrive.
        if state.probe.input_idle
            && !current.exact
            && classify > Instant::now() + Duration::from_secs(1)
        {
            classify = Instant::now() + Duration::from_millis(250);
        }
        conn.flush().map_err(|e| e.to_string())?;
        let Some(read) = conn.prepare_read() else {
            continue;
        };
        let probe_deadline = if current.exact || away.is_none() {
            Instant::now() + Duration::from_secs(86400)
        } else {
            refresh
        };
        tokio::select! {
            ready=fd.readable()=> {
                let mut ready=ready.map_err(|e|e.to_string())?;
                match read.read() {
                    Ok(_)=>ready.clear_ready(),
                    Err(wayland_client::backend::WaylandError::Io(e)) if e.kind()==std::io::ErrorKind::WouldBlock=>ready.clear_ready(),
                    Err(e)=>return Err(e.to_string()),
                }
            }
            change=settings.recv()=> {
                drop(read);
                let Some(change)=change else {break};
                if change.timeout!=current.timeout {
                    if let Some(old)=away.take(){old.destroy();}
                    let _=events.send(IdleEvent::Reset.into());
                }
                current=change;
                for old in pair.drain(..){old.destroy();}
                state.probe.normal_idle=false;state.probe.input_idle=false;refresh=Instant::now();
                classify=Instant::now()+Duration::from_secs(86400);
                let _=events.send(IdleEvent::Inference(false).into());
            }
            _=sleep_until(probe_deadline), if !current.exact && away.is_some()=> {
                drop(read);
                for old in pair.drain(..){old.destroy();}
                state.probe.normal_idle=false;state.probe.input_idle=false;
                if let (Some(notifier),Some(seat))=(&state.notifier,&state.seat) {
                    pair.push(notifier.get_idle_notification(10_000,seat,&qh,Kind::Normal));
                    pair.push(notifier.get_input_idle_notification(10_000,seat,&qh,Kind::Input));
                }
                refresh=Instant::now()+Duration::from_secs(12);
                classify=Instant::now()+Duration::from_secs(86400);
            }
            _=sleep_until(classify), if !current.exact && state.probe.input_idle=> {
                drop(read);let _=events.send(IdleEvent::Inference(state.probe.blocked().unwrap_or(false)).into());
                // At most one classification per fresh pair.
                state.probe.input_idle=false;classify=Instant::now()+Duration::from_secs(86400);
            }
            _=sleep_until(initialization), if !state.initial_done=> {return Err("Wayland registry initialization timed out".into());}
        }
    }
    if let Some(old) = away {
        old.destroy();
    }
    if let Some(old) = activity {
        old.destroy();
    }
    for old in pair {
        old.destroy();
    }
    let _ = conn.flush();
    Ok(())
}
