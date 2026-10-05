//! Always re-exec under dbus-run-session; never uses the desktop's bus or Wayland.
use desktop_idle_status::{
    adapters,
    api::{self, Row, s},
    history::{Entry, Store},
    identity::DesktopIndex,
    model::Blocker,
};
use futures_util::StreamExt;
use std::{
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use zbus::{Connection, object_server::SignalEmitter};

struct Bridge {
    effective: Arc<Mutex<bool>>,
    interface_version: Arc<Mutex<u32>>,
    activations: Arc<Mutex<u32>>,
    fail_snapshot: Arc<Mutex<bool>>,
}
#[zbus::interface(name = "io.github.StantonMatt.DesktopIdleStatus.KWinBridge1")]
impl Bridge {
    #[zbus(property)]
    fn interface_version(&self) -> u32 {
        *self.interface_version.lock().unwrap()
    }
    #[zbus(property)]
    fn built_for_k_win(&self) -> &str {
        "6.6.6"
    }
    #[zbus(property)]
    fn built_against_package(&self) -> &str {
        "test-package"
    }
    fn snapshot(&self) -> zbus::fdo::Result<(u64, Vec<Row>)> {
        if *self.fail_snapshot.lock().unwrap() {
            return Err(zbus::fdo::Error::Failed("snapshot unavailable".into()));
        }
        Ok((
            1,
            vec![Row::from([
                ("internalId".into(), s("window-1")),
                ("pid".into(), 1u32.into()),
                ("appId".into(), s("test-app")),
                ("resourceClass".into(), s("Test")),
                ("desktopFileName".into(), s("")),
                ("executablePath".into(), s("/bin/test")),
                ("caption".into(), s("Movie — Test")),
                ("effective".into(), (*self.effective.lock().unwrap()).into()),
                ("notEffectiveReason".into(), s("")),
            ])],
        ))
    }
    fn activate_window(&self, internal_id: &str) -> bool {
        *self.activations.lock().unwrap() += 1;
        internal_id == "window-1"
    }
    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>, revision: u64) -> zbus::Result<()>;
}
struct Kwin {
    version: Arc<Mutex<String>>,
}
#[zbus::interface(name = "org.kde.KWin")]
impl Kwin {
    #[zbus(name = "supportInformation")]
    fn support_information(&self) -> String {
        format!(
            "KWin Support Information:\nKWin version: {}\n",
            self.version.lock().unwrap()
        )
    }
}
type PolicyRows = Vec<(String, String, String, String, u32)>;
struct Policy {
    rows: Arc<Mutex<PolicyRows>>,
}
#[zbus::interface(name = "org.kde.Solid.PowerManagement.PolicyAgent")]
impl Policy {
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn requested_inhibitions(&self) -> Vec<(String, String, String, String, u32)> {
        self.rows.lock().unwrap().clone()
    }
    #[zbus(property(emits_changed_signal = "invalidates"))]
    fn active_inhibitions(&self) -> Vec<(String, String, String, String, u32)> {
        self.rows.lock().unwrap().clone()
    }
}
struct Power {
    profile: Arc<Mutex<String>>,
}
#[zbus::interface(name = "org.kde.Solid.PowerManagement")]
impl Power {
    #[zbus(name = "currentProfile")]
    fn current_profile(&self) -> String {
        self.profile.lock().unwrap().clone()
    }
    #[zbus(signal, name = "profileChanged")]
    async fn profile_changed(emitter: &SignalEmitter<'_>, profile: &str) -> zbus::Result<()>;
}
struct Saver;
#[zbus::interface(name = "org.kde.PlasmaVisualScreensaver")]
impl Saver {
    fn ping(&self) {}
}
struct Login;
#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Login {
    fn list_inhibitors(&self) -> Vec<(String, String, String, String, u32, u32)> {
        vec![
            (
                "idle".into(),
                "Other".into(),
                "movie".into(),
                "block".into(),
                1000,
                1,
            ),
            (
                "sleep".into(),
                "NetworkManager".into(),
                "flush".into(),
                "delay".into(),
                0,
                2,
            ),
        ]
    }
    fn can_suspend(&self) -> &str {
        "yes"
    }
}
struct LoginRecovered;
#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl LoginRecovered {
    fn list_inhibitors(&self) -> Vec<(String, String, String, String, u32, u32)> {
        vec![(
            "sleep".into(),
            "Recovered".into(),
            "recovery".into(),
            "block".into(),
            0,
            1,
        )]
    }
    fn can_suspend(&self) -> &str {
        "no"
    }
}
struct VirtualSystemd;
#[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
impl VirtualSystemd {
    #[zbus(property)]
    fn virtualization(&self) -> &str {
        "kvm"
    }
}
struct KwinSwitch {
    owner: Connection,
    replacement: Connection,
    switched: Mutex<bool>,
}
#[zbus::interface(name = "org.kde.KWin")]
impl KwinSwitch {
    #[zbus(name = "supportInformation")]
    async fn support_information(&self) -> String {
        let switch = {
            let mut done = self.switched.lock().unwrap();
            let switch = !*done;
            *done = true;
            switch
        };
        if switch {
            self.owner.release_name("org.kde.KWin").await.unwrap();
            self.replacement.request_name("org.kde.KWin").await.unwrap();
        }
        "KWin version: 6.6.6".into()
    }
}
struct Systemd;
#[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
impl Systemd {
    #[zbus(property)]
    fn virtualization(&self) -> &str {
        ""
    }
}
async fn wait_property<T>(proxy: &zbus::Proxy<'_>, property: &str, expected: T)
where
    T: for<'a> zbus::zvariant::DynamicDeserialize<'a>
        + TryFrom<zbus::zvariant::OwnedValue>
        + PartialEq
        + std::fmt::Debug,
    T::Error: Into<zbus::Error>,
{
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(value) = proxy.get_property::<T>(property).await
                && value == expected
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{property} never became {expected:?}"));
}
async fn wait_conflict(proxy: &zbus::Proxy<'_>, setting: &str, present: bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let rows: Vec<Row> = proxy.get_property("TimeoutConflicts").await.unwrap();
            if rows
                .iter()
                .any(|r| <&str>::try_from(&r["setting"]).unwrap() == setting)
                == present
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}
fn private_bus_test(name: &str) -> bool {
    if std::env::var_os("DESKTOP_IDLE_STATUS_PRIVATE_TEST").is_none() {
        let root = desktop_idle_status::paths::test_root();
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(&root).unwrap();
        let config = dir.path().join("config");
        let system_config = dir.path().join("system-config");
        std::fs::create_dir_all(&system_config).unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        for name in ["home", "cache", "state", "runtime"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.path().join("runtime"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        // Private bus address is also used for the service's system-bus adapter.
        let bus_config = dir.path().join("bus.conf");
        std::fs::write(&bus_config,format!(r#"<busconfig><type>session</type><listen>unix:tmpdir={}</listen><auth>EXTERNAL</auth><apparmor mode="disabled"/><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>"#,dir.path().display())).unwrap();
        let status=Command::new("dbus-run-session").arg("--config-file").arg(bus_config).args(["--","sh","-c","export DBUS_SYSTEM_BUS_ADDRESS=\"$DBUS_SESSION_BUS_ADDRESS\"; exec \"$1\" --exact \"$2\" --nocapture","sh"]).arg(std::env::current_exe().unwrap()).arg(name)
            .env("DESKTOP_IDLE_STATUS_PRIVATE_TEST","1").env("KDE_SKIP_KDERC","1").env("XDG_CONFIG_DIRS",format!("{}:{}", system_config.join("new-source").display(), system_config.display())).env("XDG_CONFIG_HOME",config).env("XDG_DATA_HOME",data)
            .env("HOME", dir.path().join("home")).env("XDG_CACHE_HOME", dir.path().join("cache"))
            .env("XDG_STATE_HOME", dir.path().join("state")).env("XDG_RUNTIME_DIR", dir.path().join("runtime"))
            .env_remove("DISPLAY").env_remove("XAUTHORITY").env_remove("DESKTOP_STARTUP_ID").env_remove("XDG_ACTIVATION_TOKEN")
            .env("WAYLAND_DISPLAY","no-wayland").env_remove("WAYLAND_SOCKET")
            .status().expect("dbus-run-session must be installed");
        assert!(status.success());
        return false;
    }
    true
}
#[test]
fn private_bus_contract() {
    if !private_bus_test("private_bus_contract") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let fake = Connection::session().await.unwrap();
        let effective = Arc::new(Mutex::new(true));
        let interface_version = Arc::new(Mutex::new(1));
        let activations = Arc::new(Mutex::new(0));
        let fail_snapshot = Arc::new(Mutex::new(false));
        fake.object_server()
            .at(
                "/DesktopIdleStatus",
                Bridge {
                    effective: effective.clone(),
                    interface_version: interface_version.clone(),
                    activations: activations.clone(),
                    fail_snapshot: fail_snapshot.clone(),
                },
            )
            .await
            .unwrap();
        let version = Arc::new(Mutex::new("6.6.6".to_owned()));
        fake.object_server()
            .at(
                "/KWin",
                Kwin {
                    version: version.clone(),
                },
            )
            .await
            .unwrap();
        fake.request_name("org.kde.KWin").await.unwrap();
        let policy_rows = Arc::new(Mutex::new(vec![(
            "idle".into(),
            "Other".into(),
            "movie".into(),
            "block".into(),
            3,
        )]));
        fake.object_server()
            .at(
                adapters::POLICY_PATH,
                Policy {
                    rows: policy_rows.clone(),
                },
            )
            .await
            .unwrap();
        let profile = Arc::new(Mutex::new("AC".to_owned()));
        fake.object_server()
            .at(
                adapters::POWER_PATH,
                Power {
                    profile: profile.clone(),
                },
            )
            .await
            .unwrap();
        fake.request_name("org.kde.Solid.PowerManagement")
            .await
            .unwrap();
        fake.object_server()
            .at("/PlasmaVisualScreensaver", Saver)
            .await
            .unwrap();
        fake.request_name(adapters::SAVER_NAME).await.unwrap();
        fake.object_server()
            .at("/org/freedesktop/login1", Login)
            .await
            .unwrap();
        fake.request_name("org.freedesktop.login1").await.unwrap();
        fake.object_server()
            .at("/org/freedesktop/systemd1", Systemd)
            .await
            .unwrap();
        fake.request_name("org.freedesktop.systemd1").await.unwrap();
        let index = DesktopIndex::default();
        let snapshot = adapters::bridge(&fake, &index, &[]).await.unwrap();
        assert_eq!(snapshot.blockers[0].caption, "Movie");
        let config = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());
        let config_dirs = std::env::var_os("XDG_CONFIG_DIRS").unwrap();
        let system_config = std::env::split_paths(&config_dirs).last().unwrap();
        std::fs::write(
            system_config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=1",
        )
        .unwrap();
        // Seed persistence through the same storage adapter before the service opens it.
        let data = std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
        let mut store = Store::open(&data, adapters::now()).unwrap();
        store
            .write(
                &[Entry {
                    start: adapters::now() - 120,
                    end: adapters::now() - 60,
                    blocker: Blocker {
                        internal_id: "history-window".into(),
                        app_id: "Test".into(),
                        app_name: "Test".into(),
                        icon_name: "test".into(),
                        caption: "Saved movie".into(),
                        since: 0,
                    },
                }],
                adapters::now(),
            )
            .unwrap();
        drop(store);
        let bin = data.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let command = bin.join("plasma-visual-screensaver");
        std::fs::write(
            &command,
            "#!/bin/sh\nprintf '%s' \"$1\" > \"$XDG_DATA_HOME/start-request\"\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let system_bus_link = data.join("system-bus-link");
        let mut service = Command::new(env!("CARGO_BIN_EXE_desktop-idle-status"))
            .env(
                "DBUS_SYSTEM_BUS_ADDRESS",
                format!("unix:path={}", system_bus_link.display()),
            )
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        // A guard kills only this test's process even if an assertion fails.
        struct ChildGuard<'a>(&'a mut std::process::Child);
        impl Drop for ChildGuard<'_> {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let guard = ChildGuard(&mut service);
        let client = Connection::session().await.unwrap();
        let proxy = adapters::proxy(&client, api::NAME, api::PATH, api::IFACE)
            .await
            .unwrap();
        wait_property(&proxy, "State", "blocked".to_owned()).await;
        wait_property(&proxy, "UnavailableCode", String::new()).await;
        assert!(
            proxy
                .get_property::<bool>("ExactAttribution")
                .await
                .unwrap()
        );
        let blockers: Vec<Row> = proxy.get_property("Blockers").await.unwrap();
        assert_eq!(<&str>::try_from(&blockers[0]["caption"]).unwrap(), "Movie");
        let locks: Vec<Row> = proxy.get_property("LockSleepBlockers").await.unwrap();
        assert_eq!(locks.len(), 1, "logind import must be deduplicated");
        // Initial connection failed. Restore its socket path without a bus signal.
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
        let socket = address
            .strip_prefix("unix:path=")
            .unwrap()
            .split(',')
            .next()
            .unwrap();
        std::os::unix::fs::symlink(socket, &system_bus_link).unwrap();
        fake.object_server()
            .remove::<Login, _>("/org/freedesktop/login1")
            .await
            .unwrap();
        fake.object_server()
            .at("/org/freedesktop/login1", LoginRecovered)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if proxy
                    .get_property::<Vec<Row>>("LockSleepBlockers")
                    .await
                    .unwrap()
                    .len()
                    == 2
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        fake.object_server()
            .remove::<LoginRecovered, _>("/org/freedesktop/login1")
            .await
            .unwrap();
        fake.object_server()
            .at("/org/freedesktop/login1", Login)
            .await
            .unwrap();
        fake.emit_signal(
            None::<&str>,
            "/org/freedesktop/login1",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.freedesktop.login1.Manager",
                Row::new(),
                Vec::<String>::new(),
            ),
        )
        .await
        .unwrap();

        assert!(
            proxy
                .call::<_, _, bool>("ActivateWindow", &("window-1",))
                .await
                .unwrap()
        );
        assert!(
            !proxy
                .call::<_, _, bool>("ActivateWindow", &("gone",))
                .await
                .unwrap()
        );
        let before = *activations.lock().unwrap();
        *interface_version.lock().unwrap() = 2;
        assert!(
            !proxy
                .call::<_, _, bool>("ActivateWindow", &("window-1",))
                .await
                .unwrap()
        );
        assert_eq!(
            *activations.lock().unwrap(),
            before,
            "silent incompatible replacement receives no action"
        );
        *interface_version.lock().unwrap() = 1;
        *version.lock().unwrap() = "6.7.0".into();
        assert!(
            !proxy
                .call::<_, _, bool>("ActivateWindow", &("window-1",))
                .await
                .unwrap()
        );
        assert_eq!(
            *activations.lock().unwrap(),
            before,
            "build version is also revalidated"
        );
        *version.lock().unwrap() = "6.6.6".into();
        let rows: Vec<Row> = proxy.call("History", &(7u32,)).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            <&str>::try_from(&rows[0]["caption"]).unwrap(),
            "Saved movie"
        );
        let mut changes = proxy.receive_signal("Changed").await.unwrap();
        proxy.call::<_, _, ()>("ClearHistory", &()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), changes.next())
            .await
            .unwrap()
            .unwrap();
        assert!(
            proxy
                .call::<_, _, Vec<Row>>("History", &(7u32,))
                .await
                .unwrap()
                .is_empty()
        );
        *effective.lock().unwrap() = false;
        Bridge::changed(&SignalEmitter::new(&fake, "/DesktopIdleStatus").unwrap(), 2)
            .await
            .unwrap();
        wait_property(&proxy, "State", "ready".to_owned()).await;
        // Trimming alone must emit Changed while Ready and every property is unchanged.
        while tokio::time::timeout(Duration::from_millis(50), changes.next())
            .await
            .is_ok()
        {}
        let mut external_store = Store::open(&data, adapters::now()).unwrap();
        external_store
            .write(
                &[Entry {
                    start: adapters::now() - desktop_idle_status::history::RETENTION,
                    end: adapters::now() - desktop_idle_status::history::RETENTION + 2,
                    blocker: Blocker {
                        internal_id: "expiring".into(),
                        app_id: "Test".into(),
                        app_name: "Test".into(),
                        icon_name: "test".into(),
                        caption: "Expiry".into(),
                        since: 0,
                    },
                }],
                adapters::now(),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), changes.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            proxy.get_property::<String>("State").await.unwrap(),
            "ready"
        );
        let db =
            rusqlite::Connection::open(data.join("desktop-idle-status/history.sqlite3")).unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "Changed accompanies physical retention deletion");
        drop(db);
        drop(external_store);
        // Both inherited system settings and user overrides are watched.
        std::fs::write(
            system_config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=3",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 180u32).await;
        let new_source = system_config.join("new-source");
        std::fs::create_dir(&new_source).unwrap();
        std::fs::write(
            new_source.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=4",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 240u32).await;
        std::fs::write(
            new_source.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=5",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 300u32).await;

        std::fs::write(
            config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=1",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 60u32).await;

        *version.lock().unwrap() = "6.7.0".to_owned();
        Bridge::changed(&SignalEmitter::new(&fake, "/DesktopIdleStatus").unwrap(), 3)
            .await
            .unwrap();
        wait_property(&proxy, "State", "unknown".to_owned()).await;
        wait_property(
            &proxy,
            "UnavailableCode",
            "bridge-version-mismatch".to_owned(),
        )
        .await;
        assert!(
            !proxy
                .get_property::<bool>("ExactAttribution")
                .await
                .unwrap()
        );
        assert!(
            proxy
                .get_property::<String>("UnavailableReason")
                .await
                .unwrap()
                .contains("built for 6.6.6, running 6.7.0")
        );
        *version.lock().unwrap() = "6.6.6".to_owned();
        Bridge::changed(&SignalEmitter::new(&fake, "/DesktopIdleStatus").unwrap(), 4)
            .await
            .unwrap();
        wait_property(&proxy, "State", "ready".to_owned()).await;
        wait_property(&proxy, "UnavailableCode", String::new()).await;
        wait_property(&proxy, "UnavailableReason", String::new()).await;
        // A plugin can unload without changing KWin's well-known bus owner.
        // Drain old changes, then wait without property reads: periodic revalidation
        // must invalidate the displayed state by itself.
        while tokio::time::timeout(Duration::from_millis(50), changes.next())
            .await
            .is_ok()
        {}
        fake.object_server()
            .remove::<Bridge, _>("/DesktopIdleStatus")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(7), changes.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            proxy.get_property::<String>("State").await.unwrap(),
            "unknown"
        );
        assert!(
            !proxy
                .get_property::<bool>("ExactAttribution")
                .await
                .unwrap()
        );
        wait_property(&proxy, "UnavailableCode", "bridge-missing".to_owned()).await;
        fake.object_server()
            .at(
                "/DesktopIdleStatus",
                Bridge {
                    effective: effective.clone(),
                    interface_version: interface_version.clone(),
                    activations: activations.clone(),
                    fail_snapshot: fail_snapshot.clone(),
                },
            )
            .await
            .unwrap();
        // No bridge signal is required for discovery of a newly loaded empty plugin.
        tokio::time::timeout(Duration::from_secs(7), changes.next())
            .await
            .unwrap()
            .unwrap();
        wait_property(&proxy, "State", "ready".to_owned()).await;
        assert!(
            proxy
                .get_property::<Vec<Row>>("TimeoutConflicts")
                .await
                .unwrap()
                .is_empty()
        );
        *profile.lock().unwrap() = "LowBattery".into();
        Power::profile_changed(
            &SignalEmitter::new(&fake, adapters::POWER_PATH).unwrap(),
            "LowBattery",
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let conflicts: Vec<Row> = proxy.get_property("TimeoutConflicts").await.unwrap();
                if conflicts.len() == 1
                    && <&str>::try_from(&conflicts[0]["setting"]).unwrap() == "dim"
                {
                    assert_eq!(u32::try_from(&conflicts[0]["seconds"]).unwrap(), 60);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let introspect = adapters::proxy(
            &client,
            api::NAME,
            api::PATH,
            "org.freedesktop.DBus.Introspectable",
        )
        .await
        .unwrap();
        let xml: String = introspect.call("Introspect", &()).await.unwrap();
        assert!(xml.contains("signal name=\"BlockedWhileAway\""));
        let local_data = Arc::new(Mutex::new(api::Data {
            generation: 0,
            view: desktop_idle_status::model::view(&desktop_idle_status::config::Config::read(
                &config, true, false, None,
            )),
            store: Store::open(&data.join("local"), adapters::now()).unwrap(),
            tracker: Default::default(),
            tracking_input: (false, false),
            notices: Default::default(),
        }));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        fake.object_server()
            .at(
                api::PATH,
                api::ApiInterface(api::Api {
                    index: Arc::new(DesktopIndex::load()),
                    data: local_data.clone(),
                    connection: fake.clone(),
                    changed: tx,
                }),
            )
            .await
            .unwrap();
        let local_proxy = adapters::proxy(
            &client,
            fake.unique_name().unwrap().as_str(),
            api::PATH,
            api::IFACE,
        )
        .await
        .unwrap();
        // Check handoff wire serialization on the private bus, including no delivery service.
        let mut local_returns = adapters::proxy(
            &client,
            fake.unique_name().unwrap().as_str(),
            api::PATH,
            api::IFACE,
        )
        .await
        .unwrap()
        .receive_signal("BlockedWhileAway")
        .await
        .unwrap();
        let entry = Entry {
            start: 100,
            end: 700,
            blocker: Blocker {
                internal_id: "signal-window".into(),
                app_id: "Test".into(),
                app_name: "Test".into(),
                icon_name: "test".into(),
                caption: "Movie".into(),
                since: 100,
            },
        };
        assert!(
            local_data
                .lock()
                .unwrap()
                .reserve_return(&[Entry {
                    end: 699,
                    ..entry.clone()
                }])
                .is_none()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), local_returns.next())
                .await
                .is_err()
        );
        let mut longer = entry.clone();
        longer.start = 200;
        longer.end = 900;
        longer.blocker.internal_id = "longer-window".into();
        longer.blocker.app_name = "Longest".into();
        let notice = local_data
            .lock()
            .unwrap()
            .reserve_return(&[entry.clone(), longer])
            .unwrap();
        api::publish_return(&fake, &local_data, notice)
            .await
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(3), local_returns.next())
            .await
            .unwrap()
            .unwrap();
        let (id, start, end, windows): (u32, i64, i64, Vec<Row>) =
            message.body().deserialize().unwrap();
        assert_eq!(id, 1, "subthreshold returns do not allocate IDs");
        assert!(
            !local_proxy
                .call::<_, _, bool>("ClaimReturnNotice", &(id + 1,))
                .await
                .unwrap()
        );
        // Concurrent widget clients race for the same ID; precisely one succeeds.
        let other_client = Connection::session().await.unwrap();
        let other_proxy = adapters::proxy(
            &other_client,
            fake.unique_name().unwrap().as_str(),
            api::PATH,
            api::IFACE,
        )
        .await
        .unwrap();
        let claim_args = (id,);
        let (a, b) = tokio::join!(
            local_proxy.call::<_, _, bool>("ClaimReturnNotice", &claim_args),
            other_proxy.call::<_, _, bool>("ClaimReturnNotice", &claim_args)
        );
        assert_ne!(a.unwrap(), b.unwrap());
        assert!(
            !local_proxy
                .call::<_, _, bool>("ClaimReturnNotice", &(id,))
                .await
                .unwrap()
        );
        let pending = local_data.lock().unwrap().notices.register().unwrap();
        local_proxy
            .call::<_, _, ()>("ClearHistory", &())
            .await
            .unwrap();
        assert!(
            !local_proxy
                .call::<_, _, bool>("ClaimReturnNotice", &(pending,))
                .await
                .unwrap()
        );
        // Deterministically pause at the former race boundary: consume/reserve
        // under the tracker lock, then ClearHistory while publication is delayed.
        let extracted = {
            let mut d = local_data.lock().unwrap();
            d.tracker
                .update(100, true, std::slice::from_ref(&entry.blocker));
            d.finish_intervals(700, false).unwrap();
            d.take_return().unwrap()
        };
        local_proxy
            .call::<_, _, ()>("ClearHistory", &())
            .await
            .unwrap();
        api::publish_return(&fake, &local_data, extracted)
            .await
            .unwrap();
        assert!(
            !local_proxy
                .call::<_, _, bool>("ClaimReturnNotice", &(pending + 1,))
                .await
                .unwrap()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), local_returns.next())
                .await
                .is_err()
        );
        assert!(
            local_proxy
                .call::<_, _, Vec<Row>>("History", &(7u32,))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!((start, end), (100, 900));
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].len(), 4);
        assert_eq!(u32::try_from(&windows[0]["seconds"]).unwrap(), 700);
        assert_eq!(u32::try_from(&windows[1]["seconds"]).unwrap(), 600);
        assert_eq!(<&str>::try_from(&windows[0]["appName"]).unwrap(), "Longest");
        // Snapshot failure invalidates cached attribution even without bridge signals.
        while tokio::time::timeout(Duration::from_millis(50), changes.next())
            .await
            .is_ok()
        {}
        *fail_snapshot.lock().unwrap() = true;
        tokio::time::timeout(Duration::from_secs(7), changes.next())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !proxy
                .get_property::<bool>("ExactAttribution")
                .await
                .unwrap()
        );
        assert!(
            proxy
                .get_property::<String>("UnavailableReason")
                .await
                .unwrap()
                .contains("snapshot failed")
        );
        wait_property(&proxy, "UnavailableCode", "bridge-error".to_owned()).await;
        *fail_snapshot.lock().unwrap() = false;
        tokio::time::timeout(Duration::from_secs(7), changes.next())
            .await
            .unwrap()
            .unwrap();
        wait_property(&proxy, "ExactAttribution", true).await;
        wait_property(&proxy, "UnavailableCode", String::new()).await;
        // PolicyAgent failure/recovery must update codes even with a healthy bridge.
        fake.object_server()
            .remove::<Policy, _>(adapters::POLICY_PATH)
            .await
            .unwrap();
        fake.emit_signal(
            None::<&str>,
            adapters::POLICY_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                adapters::POLICY_IFACE,
                Row::new(),
                vec!["RequestedInhibitions", "ActiveInhibitions"],
            ),
        )
        .await
        .unwrap();
        wait_property(&proxy, "State", "unknown".to_owned()).await;
        wait_property(
            &proxy,
            "UnavailableCode",
            "policyagent-unavailable".to_owned(),
        )
        .await;
        fake.object_server()
            .at(
                adapters::POLICY_PATH,
                Policy {
                    rows: policy_rows.clone(),
                },
            )
            .await
            .unwrap();
        // No recovery signal: failed PolicyAgent reads must retry.
        wait_property(&proxy, "UnavailableCode", String::new()).await;
        wait_property(&proxy, "UnavailableReason", String::new()).await;
        fake.object_server()
            .remove::<Power, _>(adapters::POWER_PATH)
            .await
            .unwrap();
        fake.emit_signal(
            None::<&str>,
            adapters::POWER_PATH,
            adapters::POWER_IFACE,
            "configurationReloaded",
            &(),
        )
        .await
        .unwrap();
        wait_property(&proxy, "TimeoutConflicts", Vec::<Row>::new()).await;
        fake.object_server()
            .at(
                adapters::POWER_PATH,
                Power {
                    profile: profile.clone(),
                },
            )
            .await
            .unwrap();
        // Removing a parent object can remove its PolicyAgent child in the fake server.
        fake.object_server()
            .at(
                adapters::POLICY_PATH,
                Policy {
                    rows: policy_rows.clone(),
                },
            )
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if !proxy
                    .get_property::<Vec<Row>>("TimeoutConflicts")
                    .await
                    .unwrap()
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        std::fs::write(
            config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=240",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 14400u32).await;
        wait_conflict(&proxy, "suspend", true).await;
        fake.object_server()
            .remove::<Login, _>("/org/freedesktop/login1")
            .await
            .unwrap();
        fake.emit_signal(
            None::<&str>,
            "/org/freedesktop/login1",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.freedesktop.login1.Manager",
                Row::new(),
                Vec::<String>::new(),
            ),
        )
        .await
        .unwrap();
        // Add a unique logind row on recovery, without a signal.
        tokio::time::sleep(Duration::from_millis(150)).await;
        fake.object_server()
            .at("/org/freedesktop/login1", LoginRecovered)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if proxy
                    .get_property::<Vec<Row>>("LockSleepBlockers")
                    .await
                    .unwrap()
                    .len()
                    == 2
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        wait_conflict(&proxy, "suspend", false).await; // CanSuspend also recovers without a signal.
        fake.object_server()
            .remove::<LoginRecovered, _>("/org/freedesktop/login1")
            .await
            .unwrap();
        fake.object_server()
            .at("/org/freedesktop/login1", Login)
            .await
            .unwrap();
        fake.emit_signal(
            None::<&str>,
            "/org/freedesktop/login1",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.freedesktop.login1.Manager",
                Row::new(),
                Vec::<String>::new(),
            ),
        )
        .await
        .unwrap();

        wait_conflict(&proxy, "suspend", true).await;
        // Failed Virtualization recovers silently; successful CanSuspend is retained.
        std::fs::write(
            config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=240",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 14400u32).await;
        fake.object_server()
            .remove::<Systemd, _>("/org/freedesktop/systemd1")
            .await
            .unwrap();
        fake.emit_signal(
            None::<&str>,
            "/org/freedesktop/systemd1",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.freedesktop.systemd1.Manager",
                Row::new(),
                Vec::<String>::new(),
            ),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        fake.object_server()
            .at("/org/freedesktop/systemd1", VirtualSystemd)
            .await
            .unwrap();
        wait_conflict(&proxy, "suspend", false).await;
        fake.object_server()
            .remove::<VirtualSystemd, _>("/org/freedesktop/systemd1")
            .await
            .unwrap();
        fake.object_server()
            .at("/org/freedesktop/systemd1", Systemd)
            .await
            .unwrap();
        fake.emit_signal(
            None::<&str>,
            "/org/freedesktop/systemd1",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                "org.freedesktop.systemd1.Manager",
                Row::new(),
                Vec::<String>::new(),
            ),
        )
        .await
        .unwrap();
        std::fs::write(
            config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=1",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 60u32).await;
        // PropertiesChanged invalidations must trigger a reread, not require inline values.
        policy_rows.lock().unwrap().push((
            "idle".into(),
            "org.kde.plasmavisualscreensaver".into(),
            "Visual screensaver is active".into(),
            "block".into(),
            3,
        ));
        fake.emit_signal(
            None::<&str>,
            adapters::POLICY_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                adapters::POLICY_IFACE,
                Row::new(),
                vec!["RequestedInhibitions", "ActiveInhibitions"],
            ),
        )
        .await
        .unwrap();
        wait_property(&proxy, "State", "running".to_owned()).await;
        assert!(proxy.get_property::<i64>("RunningSince").await.unwrap() > 0);
        assert_eq!(
            proxy
                .get_property::<Vec<Row>>("LockSleepBlockers")
                .await
                .unwrap()
                .len(),
            1
        );

        // Transfer the well-known owner in the middle of compatibility validation.
        let replacement = Connection::session().await.unwrap();
        let replacement_activations = Arc::new(Mutex::new(0));
        replacement
            .object_server()
            .at(
                "/DesktopIdleStatus",
                Bridge {
                    effective: effective.clone(),
                    interface_version: interface_version.clone(),
                    activations: replacement_activations.clone(),
                    fail_snapshot: fail_snapshot.clone(),
                },
            )
            .await
            .unwrap();
        replacement
            .object_server()
            .at(
                "/KWin",
                Kwin {
                    version: version.clone(),
                },
            )
            .await
            .unwrap();
        fake.object_server()
            .remove::<Kwin, _>("/KWin")
            .await
            .unwrap();
        fake.object_server()
            .at(
                "/KWin",
                KwinSwitch {
                    owner: fake.clone(),
                    replacement: replacement.clone(),
                    switched: Mutex::new(false),
                },
            )
            .await
            .unwrap();
        let old_count = *activations.lock().unwrap();
        assert!(adapters::activate_window(&fake, "window-1").await.is_err());
        assert_eq!(*activations.lock().unwrap(), old_count);
        assert_eq!(
            *replacement_activations.lock().unwrap(),
            0,
            "replacement never receives a redirected action"
        );
        replacement.release_name("org.kde.KWin").await.unwrap();
        fake.object_server()
            .remove::<KwinSwitch, _>("/KWin")
            .await
            .unwrap();
        fake.object_server()
            .at(
                "/KWin",
                Kwin {
                    version: version.clone(),
                },
            )
            .await
            .unwrap();
        fake.request_name("org.kde.KWin").await.unwrap();
        wait_property(&proxy, "ExactAttribution", true).await;
        fake.release_name("org.kde.KWin").await.unwrap();
        wait_property(&proxy, "ExactAttribution", false).await;
        fake.request_name("org.kde.KWin").await.unwrap();
        wait_property(&proxy, "ExactAttribution", true).await;
        fake.release_name("org.kde.KWin").await.unwrap();
        wait_property(&proxy, "ExactAttribution", false).await;
        policy_rows.lock().unwrap().retain(|p| p.1 == "Other");
        fake.emit_signal(
            None::<&str>,
            adapters::POLICY_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                adapters::POLICY_IFACE,
                Row::new(),
                vec!["RequestedInhibitions", "ActiveInhibitions"],
            ),
        )
        .await
        .unwrap();
        wait_property(&proxy, "State", "unknown".to_owned()).await;
        assert!(
            !proxy
                .call::<_, _, bool>("ActivateWindow", &("window-1",))
                .await
                .unwrap()
        );
        fake.release_name(adapters::SAVER_NAME).await.unwrap();
        wait_property(&proxy, "State", "screensaver-off".to_owned()).await;
        std::fs::write(
            config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=2",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 120u32).await;
        std::fs::write(
            new_source.join("plasma-visual-screensaverrc"),
            "[General][$i]\nIdleMinutes=1",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 60u32).await;
        std::fs::write(
            new_source.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=5",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 120u32).await;

        assert!(
            proxy
                .call::<_, _, bool>("StartScreensaver", &())
                .await
                .unwrap()
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while std::fs::read_to_string(data.join("start-request"))
                .ok()
                .as_deref()
                != Some("--background")
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(data.join("start-request")).unwrap(),
            "--background"
        );
        assert!(
            Command::new("kill")
                .args(["-TERM", &guard.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        assert!(guard.0.wait().unwrap().success());
        drop(guard);
    });
}

#[test]
fn private_bus_single_instance() {
    if !private_bus_test("private_bus_single_instance") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use zbus::fdo::{DBusProxy, RequestNameFlags};
        let client = Connection::session().await.unwrap();
        let dbus = DBusProxy::new(&client).await.unwrap();
        let mut first = ServiceChild(
            Command::new(env!("CARGO_BIN_EXE_desktop-idle-status"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let proxy = adapters::proxy(&client, api::NAME, api::PATH, api::IFACE)
            .await
            .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 600u32).await;
        let name = api::NAME.try_into().unwrap();
        let owner = dbus.get_name_owner(name).await.unwrap();
        // Isolate DB to exercise name rejection independently of the DB lock.
        let data = std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
        let second_data = data.join("second");
        let mut second = ServiceChild(
            Command::new(env!("CARGO_BIN_EXE_desktop-idle-status"))
                .env("XDG_DATA_HOME", &second_data)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let status = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if let Some(status) = second.0.try_wait().unwrap() {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second service must reject ownership promptly");
        assert!(!status.success());
        use std::io::Read;
        let mut log = String::new();
        second
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut log)
            .unwrap();
        assert!(
            log.contains("Cannot own") && log.contains("another instance"),
            "{log}"
        );
        assert_eq!(
            dbus.get_name_owner(api::NAME.try_into().unwrap())
                .await
                .unwrap(),
            owner
        );
        // Even an explicitly replacing peer cannot evict the production owner.
        assert!(
            client
                .request_name_with_flags(
                    api::NAME,
                    RequestNameFlags::ReplaceExisting | RequestNameFlags::DoNotQueue
                )
                .await
                .is_err()
        );
        assert!(first.0.try_wait().unwrap().is_none());
        // The same database is guarded for smoke and across separate bus connections.
        let mut smoke = ServiceChild(
            Command::new(env!("CARGO_BIN_EXE_desktop-idle-status"))
                .arg("--smoke")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let status = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if let Some(status) = smoke.0.try_wait().unwrap() {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!status.success());
        let mut log = String::new();
        smoke
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut log)
            .unwrap();
        assert!(log.contains("Cannot lock history database"), "{log}");
        let config = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());
        std::fs::write(
            config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=2",
        )
        .unwrap();
        wait_property(&proxy, "ScreensaverTimeout", 120u32).await;
        stop_within_budget(&mut first, "-TERM").await;
        // Kernel releases the advisory lock on shutdown, without unlinking it.
        let _lock = desktop_idle_status::history::lock_database(&data).unwrap();
    });
}

#[test]
fn private_bus_name_lost_monitor() {
    if !private_bus_test("private_bus_name_lost_monitor") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use desktop_idle_status::ownership;
        use zbus::fdo::RequestNameFlags;
        let owner = Connection::session().await.unwrap();
        let stream = ownership::loss_stream(&owner).await.unwrap();
        // Only this test peer allows replacement to exercise real NameLost.
        owner
            .request_name_with_flags(
                api::NAME,
                RequestNameFlags::AllowReplacement | RequestNameFlags::DoNotQueue,
            )
            .await
            .unwrap();
        let replacement = Connection::session().await.unwrap();
        replacement
            .request_name_with_flags(
                api::NAME,
                RequestNameFlags::ReplaceExisting | RequestNameFlags::DoNotQueue,
            )
            .await
            .unwrap();
        let reason = tokio::time::timeout(Duration::from_secs(2), ownership::wait_for_loss(stream))
            .await
            .unwrap();
        assert!(reason.contains("Lost D-Bus name") && reason.contains(api::NAME));
    });
}

// Minimal protocol fixture: a private socket, registry, seat and ext-idle-notify
// objects only. It never creates or connects to a desktop compositor.
#[derive(Clone, Copy)]
enum Detector {
    Away,
    Activity,
    Normal,
    Input,
}
enum FixtureCommand {
    Idle(Detector),
    Resume(Detector),
    Disconnect,
}
struct IdleFixture {
    commands: std::sync::mpsc::Sender<FixtureCommand>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl IdleFixture {
    fn start(path: &std::path::Path) -> (Self, tokio::sync::oneshot::Receiver<()>) {
        let (fixture, away, _) = Self::controlled(path, true);
        (fixture, away)
    }
    fn controlled(
        path: &std::path::Path,
        auto_away: bool,
    ) -> (
        Self,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::mpsc::UnboundedReceiver<(u32, u32, u32)>,
    ) {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixListener,
            sync::atomic::{AtomicBool, Ordering},
        };
        let listener = UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (commands, command_rx) = std::sync::mpsc::channel();
        let (created_tx, created_rx) = tokio::sync::mpsc::unbounded_channel();
        let thread = std::thread::spawn(move || {
            let mut tx = Some(tx);
            let mut stream = loop {
                if stopping.load(Ordering::Relaxed) {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("private Wayland accept: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_millis(10)))
                .unwrap();
            fn event(stream: &mut std::os::unix::net::UnixStream, id: u32, op: u32, body: &[u8]) {
                let mut bytes = id.to_ne_bytes().to_vec();
                bytes.extend_from_slice(&(((body.len() as u32 + 8) << 16) | op).to_ne_bytes());
                bytes.extend_from_slice(body);
                // A cancelled service may close while an event is being sent.
                let _ = stream.write_all(&bytes);
            }
            fn global(
                stream: &mut std::os::unix::net::UnixStream,
                registry: u32,
                name: u32,
                interface: &str,
                version: u32,
            ) {
                let mut body = name.to_ne_bytes().to_vec();
                body.extend_from_slice(&(interface.len() as u32 + 1).to_ne_bytes());
                body.extend_from_slice(interface.as_bytes());
                body.push(0);
                while !body.len().is_multiple_of(4) {
                    body.push(0);
                }
                body.extend_from_slice(&version.to_ne_bytes());
                event(stream, registry, 0, &body);
            }
            let mut buffer = Vec::new();
            let mut registry = 0;
            let mut notifier = 0;
            let mut detectors = std::collections::HashMap::<u32, (u32, u32)>::new();
            while !stopping.load(Ordering::Relaxed) {
                while let Ok(command) = command_rx.try_recv() {
                    let (kind, opcode) = match command {
                        FixtureCommand::Idle(kind) => (kind, 0),
                        FixtureCommand::Resume(kind) => (kind, 1),
                        FixtureCommand::Disconnect => return,
                    };
                    let wanted = match kind {
                        Detector::Away => (2, 60_000),
                        Detector::Activity => (2, 10_000),
                        Detector::Normal => (1, 10_000),
                        Detector::Input => (2, 10_000),
                    };
                    // Activity is the first persistent input 10 s detector;
                    // the newest such detector belongs to the paired probe.
                    let mut ids = detectors
                        .iter()
                        .filter(|(_, v)| **v == wanted)
                        .map(|(id, _)| *id)
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    let id = if matches!(kind, Detector::Activity) {
                        ids.first()
                    } else {
                        ids.last()
                    };
                    if let Some(id) = id {
                        event(&mut stream, *id, opcode, &[]);
                    }
                }
                let mut chunk = [0; 4096];
                match stream.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => return,
                }
                while buffer.len() >= 8 {
                    let word =
                        |offset| u32::from_ne_bytes(buffer[offset..offset + 4].try_into().unwrap());
                    let id = word(0);
                    let header = word(4);
                    let op = header & 0xffff;
                    let size = (header >> 16) as usize;
                    assert!(size >= 8);
                    if buffer.len() < size {
                        break;
                    }
                    if id == 1 && op == 1 {
                        registry = word(8);
                        global(&mut stream, registry, 1, "wl_seat", 7);
                        global(&mut stream, registry, 2, "ext_idle_notifier_v1", 2);
                    } else if id == 1 && op == 0 {
                        let callback = word(8);
                        event(&mut stream, callback, 0, &0u32.to_ne_bytes());
                        event(&mut stream, 1, 1, &callback.to_ne_bytes());
                    } else if id == registry && op == 0 && word(8) == 2 {
                        notifier = word(size - 4);
                    } else if id == notifier && matches!(op, 1 | 2) {
                        let detector = word(8);
                        let timeout = word(12);
                        detectors.insert(detector, (op, timeout));
                        let _ = created_tx.send((op, timeout, detector));
                        if auto_away && op == 2 && timeout == 60_000 {
                            event(&mut stream, detector, 0, &[]);
                            if let Some(tx) = tx.take() {
                                let _ = tx.send(());
                            }
                        }
                    } else if detectors.contains_key(&id) && op == 0 {
                        detectors.remove(&id);
                        event(&mut stream, 1, 1, &id.to_ne_bytes());
                    }
                    buffer.drain(..size);
                }
            }
        });
        (
            Self {
                commands,
                stop,
                thread: Some(thread),
            },
            rx,
            created_rx,
        )
    }
}
impl Drop for IdleFixture {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
struct SlowPolicy {
    stall: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
}
#[zbus::interface(name = "org.kde.Solid.PowerManagement.PolicyAgent")]
impl SlowPolicy {
    #[zbus(property)]
    async fn requested_inhibitions(&self) -> PolicyRows {
        if self.stall.load(std::sync::atomic::Ordering::Relaxed) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        vec![]
    }
    #[zbus(property)]
    async fn active_inhibitions(&self) -> PolicyRows {
        if self.stall.load(std::sync::atomic::Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
        vec![]
    }
}
struct SlowPower(Arc<std::sync::atomic::AtomicBool>);
#[zbus::interface(name = "org.kde.Solid.PowerManagement")]
impl SlowPower {
    #[zbus(name = "currentProfile")]
    async fn current_profile(&self) -> &str {
        if self.0.load(std::sync::atomic::Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
        "AC"
    }
}
struct ServiceChild(std::process::Child);
impl Drop for ServiceChild {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
async fn stop_within_budget(child: &mut ServiceChild, signal: &str) {
    assert!(
        Command::new("kill")
            .args([signal, &child.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let started = tokio::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("shutdown must beat TimeoutStopSec=5");
    assert!(started.elapsed() < Duration::from_secs(4));
}
#[test]
fn private_bus_shutdown() {
    if !private_bus_test("private_bus_shutdown") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let config = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());
        std::fs::write(
            config.join("plasma-visual-screensaverrc"),
            "[General]\nIdleMinutes=1",
        )
        .unwrap();
        let socket = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join("idle-fixture");
        let (_fixture, away_sent) = IdleFixture::start(&socket);
        let fake = Connection::session().await.unwrap();
        fake.object_server()
            .at(
                "/DesktopIdleStatus",
                Bridge {
                    effective: Arc::new(Mutex::new(true)),
                    interface_version: Arc::new(Mutex::new(1)),
                    activations: Default::default(),
                    fail_snapshot: Default::default(),
                },
            )
            .await
            .unwrap();
        fake.object_server()
            .at(
                "/KWin",
                Kwin {
                    version: Arc::new(Mutex::new("6.6.6".into())),
                },
            )
            .await
            .unwrap();
        fake.request_name("org.kde.KWin").await.unwrap();
        let stall = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        fake.object_server()
            .at(
                adapters::POLICY_PATH,
                SlowPolicy {
                    stall: stall.clone(),
                    entered: entered.clone(),
                },
            )
            .await
            .unwrap();
        fake.object_server()
            .at(adapters::POWER_PATH, SlowPower(stall.clone()))
            .await
            .unwrap();
        fake.request_name("org.kde.Solid.PowerManagement")
            .await
            .unwrap();
        fake.request_name(adapters::SAVER_NAME).await.unwrap();
        let mut service = ServiceChild(
            Command::new(env!("CARGO_BIN_EXE_desktop-idle-status"))
                .env("WAYLAND_DISPLAY", &socket)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(8), away_sent)
            .await
            .unwrap()
            .unwrap();
        let client = Connection::session().await.unwrap();
        let proxy = adapters::proxy(&client, api::NAME, api::PATH, api::IFACE)
            .await
            .unwrap();
        wait_property(&proxy, "State", "blocked".to_owned()).await;
        // Real clock and real active tracker interval: no production test hooks.
        // Give the event loop time to process Away, then qualify the 60 s interval.
        tokio::time::sleep(Duration::from_secs(63)).await;
        let data = std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
        assert!(
            Store::open(&data, adapters::now())
                .unwrap()
                .history(7, adapters::now())
                .unwrap()
                .is_empty()
        );
        stall.store(true, std::sync::atomic::Ordering::Relaxed);
        fake.emit_signal(
            None::<&str>,
            adapters::POLICY_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(
                adapters::POLICY_IFACE,
                Row::new(),
                vec!["RequestedInhibitions"],
            ),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(8), entered.notified())
            .await
            .unwrap();
        let mut changes = proxy.receive_signal("Changed").await.unwrap();
        let db = rusqlite::Connection::open(data.join("desktop-idle-status/history.sqlite3"))
            .unwrap();
        db.execute_batch("CREATE TRIGGER reject BEFORE INSERT ON history BEGIN SELECT RAISE(FAIL,'injected shutdown write failure'); END;").unwrap();
        // Release the injected failure only after shutdown's first flush really
        // failed, so this checks publication from the retry rather than timing.
        let write_failed = Arc::new(tokio::sync::Notify::new());
        let failed = write_failed.clone();
        let stderr = service.0.stderr.take().unwrap();
        let logs = tokio::task::spawn_blocking(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines() {
                let line = line.unwrap();
                if line.contains("History write failed;") {
                    failed.notify_one();
                }
            }
        });
        let recover = async {
            tokio::time::timeout(Duration::from_secs(2), write_failed.notified())
                .await
                .expect("shutdown must attempt the failing write");
            db.execute_batch("DROP TRIGGER reject;").unwrap();
        };
        tokio::join!(stop_within_budget(&mut service, "-TERM"), recover);
        logs.await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), changes.next())
            .await
            .expect("shutdown retry must publish its history commit")
            .unwrap();
        let rows = Store::open(&data, adapters::now())
            .unwrap()
            .history(7, adapters::now())
            .unwrap();
        assert_eq!(rows.len(), 1, "SIGTERM must flush the active interval");
        assert_eq!(rows[0].blocker.internal_id, "window-1");
        assert!(rows[0].end - rows[0].start >= 60);
        // Smoke completion also cancels stalled refreshes at its own deadline.
        let started = tokio::time::Instant::now();
        let mut smoke = ServiceChild(
            Command::new(env!("CARGO_BIN_EXE_desktop-idle-status"))
                .arg("--smoke")
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(17), async {
            loop {
                if let Some(status) = smoke.0.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("smoke deadline must cancel I/O too");
        assert!(started.elapsed() < Duration::from_secs(17));
        use std::io::Read;
        let mut snapshot = String::new();
        smoke
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut snapshot)
            .unwrap();
        assert!(snapshot.contains("ScreensaverTimeout=60"));
        // Drain permits from smoke retries before testing SIGINT's first read.
        while tokio::time::timeout(Duration::from_millis(1), entered.notified())
            .await
            .is_ok()
        {}
        // SIGINT uses the same cancellation/flush path during a stalled refresh,
        // including an unavailable Wayland reconnect/initialization path.
        let mut service = ServiceChild(
            Command::new(env!("CARGO_BIN_EXE_desktop-idle-status"))
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(8), entered.notified())
            .await
            .unwrap();
        stop_within_budget(&mut service, "-INT").await;
        assert_eq!(
            Store::open(&data, adapters::now())
                .unwrap()
                .history(7, adapters::now())
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn private_wayland_paired_transitions() {
    if !private_bus_test("private_wayland_paired_transitions") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use desktop_idle_status::wayland::{self, IdleEvent, ObservedEvent, Settings};
        async fn next(
            rx: &mut tokio::sync::mpsc::UnboundedReceiver<ObservedEvent>,
        ) -> ObservedEvent {
            tokio::time::timeout(Duration::from_secs(3), rx.recv())
                .await
                .unwrap()
                .unwrap()
        }
        async fn pair(rx: &mut tokio::sync::mpsc::UnboundedReceiver<(u32, u32, u32)>) {
            loop {
                let (op, timeout, _) = tokio::time::timeout(Duration::from_secs(14), rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                if op == 1 && timeout == 10_000 {
                    break;
                }
            }
            let (op, timeout, _) = rx.recv().await.unwrap();
            assert_eq!((op, timeout), (2, 10_000));
        }
        let socket = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(std::env::var_os("WAYLAND_DISPLAY").unwrap());
        let (fixture, _away, mut created) = IdleFixture::controlled(&socket, false);
        let (tx, settings) = tokio::sync::mpsc::unbounded_channel();
        let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(Settings {
            timeout: 60,
            exact: false,
        })
        .unwrap();
        let task = tokio::spawn(wayland::run(settings, events));
        assert!(matches!(next(&mut rx).await.event, IdleEvent::Available));
        pair(&mut created).await;
        fixture
            .commands
            .send(FixtureCommand::Idle(Detector::Away))
            .unwrap();
        let before = adapters::now();
        let away = next(&mut rx).await;
        assert!(matches!(away.event, IdleEvent::Away));
        assert!(away.at >= before && away.at <= adapters::now());
        fixture
            .commands
            .send(FixtureCommand::Idle(Detector::Input))
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), rx.recv())
                .await
                .is_err(),
            "grace must let the normal event arrive"
        );
        fixture
            .commands
            .send(FixtureCommand::Idle(Detector::Normal))
            .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(false)
        ));
        // A fresh pair must infer inhibition from input-only idling.
        tx.send(Settings {
            timeout: 60,
            exact: false,
        })
        .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(false)
        ));
        pair(&mut created).await;
        fixture
            .commands
            .send(FixtureCommand::Idle(Detector::Input))
            .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(true)
        ));
        fixture
            .commands
            .send(FixtureCommand::Resume(Detector::Input))
            .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(false)
        ));
        fixture
            .commands
            .send(FixtureCommand::Idle(Detector::Activity))
            .unwrap();
        fixture
            .commands
            .send(FixtureCommand::Resume(Detector::Activity))
            .unwrap();
        assert!(matches!(next(&mut rx).await.event, IdleEvent::Resumed));
        fixture
            .commands
            .send(FixtureCommand::Resume(Detector::Away))
            .unwrap();
        assert!(matches!(next(&mut rx).await.event, IdleEvent::Resumed));
        // The timer creates fresh probes even without a settings signal.
        pair(&mut created).await;
        fixture
            .commands
            .send(FixtureCommand::Idle(Detector::Input))
            .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(true)
        ));
        tx.send(Settings {
            timeout: 90,
            exact: true,
        })
        .unwrap();
        assert!(matches!(next(&mut rx).await.event, IdleEvent::Reset));
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(false)
        ));
        assert!(matches!(next(&mut rx).await.event, IdleEvent::Available));
        // Away timeout is recreated; exact mode suppresses paired probes.
        let (_, timeout, _) = created.recv().await.unwrap();
        assert_eq!(timeout, 90_000);
        assert!(
            tokio::time::timeout(Duration::from_millis(400), created.recv())
                .await
                .is_err()
        );
        tx.send(Settings {
            timeout: 90,
            exact: false,
        })
        .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(false)
        ));
        pair(&mut created).await;
        fixture.commands.send(FixtureCommand::Disconnect).unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Unavailable(_)
        ));
        drop(fixture);
        std::fs::remove_file(&socket).unwrap();
        let (recovered, _away, mut created) = IdleFixture::controlled(&socket, false);
        tx.send(Settings {
            timeout: 60,
            exact: false,
        })
        .unwrap();
        assert!(matches!(next(&mut rx).await.event, IdleEvent::Available));
        pair(&mut created).await;
        recovered
            .commands
            .send(FixtureCommand::Idle(Detector::Input))
            .unwrap();
        assert!(matches!(
            next(&mut rx).await.event,
            IdleEvent::Inference(true)
        ));
        drop(tx);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        drop(recovered);
        std::fs::remove_file(socket).unwrap();
    });
}

struct ValidationBridge {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    stall: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
#[zbus::interface(name = "io.github.StantonMatt.DesktopIdleStatus.KWinBridge1")]
impl ValidationBridge {
    #[zbus(property)]
    fn interface_version(&self) -> u32 {
        1
    }
    #[zbus(property)]
    fn built_for_k_win(&self) -> &str {
        "6.6.6"
    }
    #[zbus(property)]
    fn built_against_package(&self) -> &str {
        "test"
    }
    async fn snapshot(&self) -> zbus::fdo::Result<(u64, Vec<Row>)> {
        use std::sync::atomic::Ordering;
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.stall.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
            return Err(zbus::fdo::Error::Failed("old owner unloaded".into()));
        }
        Ok((1, vec![]))
    }
}
#[test]
fn private_bus_coherent_validation() {
    if !private_bus_test("private_bus_coherent_validation") {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let fake = Connection::session().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let stall = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        fake.object_server()
            .at(
                "/DesktopIdleStatus",
                ValidationBridge {
                    calls: calls.clone(),
                    stall: stall.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                },
            )
            .await
            .unwrap();
        fake.object_server()
            .at(
                "/KWin",
                Kwin {
                    version: Arc::new(Mutex::new("6.6.6".into())),
                },
            )
            .await
            .unwrap();
        fake.request_name("org.kde.KWin").await.unwrap();
        let config = desktop_idle_status::config::Config {
            timeout: 60,
            conflicts: vec![],
        };
        let mut ready = desktop_idle_status::model::view(&config);
        ready.state = "ready".into();
        ready.exact = true;
        ready.unavailable_code.clear();
        ready.unavailable_reason.clear();
        let data_home = std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap());
        let data = Arc::new(Mutex::new(api::Data {
            view: ready.clone(),
            generation: 0,
            store: Store::open(&data_home, adapters::now()).unwrap(),
            tracker: Default::default(),
            tracking_input: (false, false),
            notices: Default::default(),
        }));
        let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
        fake.object_server()
            .at(
                api::PATH,
                api::ApiInterface(api::Api {
                    index: Arc::new(DesktopIndex::load()),
                    data: data.clone(),
                    connection: fake.clone(),
                    changed,
                }),
            )
            .await
            .unwrap();
        fake.request_name(api::NAME).await.unwrap();
        let client = Connection::session().await.unwrap();
        let props = adapters::proxy(
            &client,
            api::NAME,
            api::PATH,
            "org.freedesktop.DBus.Properties",
        )
        .await
        .unwrap();
        let rows: Row = props.call("GetAll", &(api::IFACE,)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one validation per GetAll");
        assert_eq!(<&str>::try_from(&rows["State"]).unwrap(), "ready");
        assert!(bool::try_from(&rows["ExactAttribution"]).unwrap());
        fake.object_server()
            .remove::<ValidationBridge, _>("/DesktopIdleStatus")
            .await
            .unwrap();
        let rows: Row = props.call("GetAll", &(api::IFACE,)).await.unwrap();
        assert_eq!(<&str>::try_from(&rows["State"]).unwrap(), "unknown");
        assert!(!bool::try_from(&rows["ExactAttribution"]).unwrap());
        changes.recv().await.unwrap();
        fake.object_server()
            .at(
                "/DesktopIdleStatus",
                ValidationBridge {
                    calls: calls.clone(),
                    stall: stall.clone(),
                    entered: entered.clone(),
                    release: release.clone(),
                },
            )
            .await
            .unwrap();
        data.lock().unwrap().set_view(ready.clone());
        stall.store(true, Ordering::SeqCst);
        let request = tokio::spawn(async move {
            props
                .call::<_, _, Row>("GetAll", &(api::IFACE,))
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        {
            let mut d = data.lock().unwrap();
            let mut unavailable = ready.clone();
            unavailable.exact = false;
            unavailable.state = "unknown".into();
            d.set_view(unavailable);
            d.set_view(ready); // ABA: values identical, observation generation differs.
        }
        release.notify_one();
        let rows = tokio::time::timeout(Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(<&str>::try_from(&rows["State"]).unwrap(), "ready");
        assert!(bool::try_from(&rows["ExactAttribution"]).unwrap());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "retry only the stale validation"
        );
        assert!(data.lock().unwrap().view.exact);
        assert!(
            changes.try_recv().is_err(),
            "old failure must not invalidate recovered observation"
        );

        // A property validation also retries storage. No view transition is
        // needed for another widget to receive the newly committed history.
        let props = adapters::proxy(
            &client,
            api::NAME,
            api::PATH,
            "org.freedesktop.DBus.Properties",
        )
        .await
        .unwrap();
        let widget = Connection::session().await.unwrap();
        let widget_proxy = adapters::proxy(&widget, api::NAME, api::PATH, api::IFACE)
            .await
            .unwrap();
        let mut widget_changes = widget_proxy.receive_signal("Changed").await.unwrap();
        let db = rusqlite::Connection::open(
            data_home.join("desktop-idle-status/history.sqlite3"),
        )
        .unwrap();
        for method in ["GetAll", "Get"] {
            let at = adapters::now();
            let previous = data.lock().unwrap().view.clone();
            db.execute_batch("CREATE TRIGGER reject BEFORE INSERT ON history BEGIN SELECT RAISE(FAIL,'injected write failure'); END;").unwrap();
            {
                let mut d = data.lock().unwrap();
                d.tracker.update(
                    at - 600,
                    true,
                    &[Blocker {
                        internal_id: method.into(),
                        app_id: "test-app".into(),
                        app_name: "Test".into(),
                        icon_name: "test".into(),
                        caption: "pending".into(),
                        since: at - 600,
                    }],
                );
                assert!(d.finish_intervals(at, true).is_err());
                assert_eq!(d.tracker.pending().len(), 1);
            }
            for retry in [false, true] {
                if retry {
                    db.execute_batch("DROP TRIGGER reject;").unwrap();
                }
                if method == "GetAll" {
                    let rows: Row = props.call(method, &(api::IFACE,)).await.unwrap();
                    assert_eq!(<&str>::try_from(&rows["State"]).unwrap(), "ready");
                } else {
                    let state: zbus::zvariant::OwnedValue =
                        props.call(method, &(api::IFACE, "State")).await.unwrap();
                    assert_eq!(<&str>::try_from(&state).unwrap(), "ready");
                }
                assert_eq!(data.lock().unwrap().view, previous);
                if retry {
                    tokio::time::timeout(Duration::from_secs(2), changes.recv())
                        .await
                        .expect("history-only retry must queue publication")
                        .unwrap();
                    // Same publication path as the daemon's changed receiver.
                    api::publish(&fake).await.unwrap();
                    tokio::time::timeout(Duration::from_secs(2), widget_changes.next())
                        .await
                        .unwrap()
                        .unwrap();
                    let rows: Vec<Row> = widget_proxy.call("History", &(7u32,)).await.unwrap();
                    assert_eq!(rows.len(), 1);
                    assert_eq!(<&str>::try_from(&rows[0]["internalId"]).unwrap(), method);
                    assert!(data.lock().unwrap().tracker.pending().is_empty());
                } else {
                    assert!(changes.try_recv().is_err(), "failed retry is not a commit");
                    assert_eq!(data.lock().unwrap().tracker.pending().len(), 1);
                }
            }
            let _: Row = props.call("GetAll", &(api::IFACE,)).await.unwrap();
            assert!(changes.try_recv().is_err(), "a no-op must not publish again");

            // An unchanged bridge can trim history without inserting anything.
            let end = at - desktop_idle_status::history::RETENTION - 1;
            db.execute(
                "INSERT INTO history VALUES (?1,?2,'expired','test','Test','test','expired')",
                rusqlite::params![end - 60, end],
            )
            .unwrap();
            let _: Row = props.call("GetAll", &(api::IFACE,)).await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), changes.recv())
                .await
                .expect("retention-only validation must queue publication")
                .unwrap();
            assert_eq!(data.lock().unwrap().view, previous);
            let count: i64 = db.query_row("SELECT COUNT(*) FROM history", [], |r| r.get(0)).unwrap();
            assert_eq!(count, 1);
            widget_proxy.call::<_, _, ()>("ClearHistory", &()).await.unwrap();
            changes.recv().await.unwrap();
        }
    });
}
