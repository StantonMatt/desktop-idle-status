//! Read-only desktop adapters; only activate/start are user-requested writes.
use crate::{
    api::Row,
    identity::DesktopIndex,
    model::{Blocker, Policy, is_screensaver},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zbus::{Connection, Proxy};

pub const BRIDGE_IFACE: &str = "io.github.StantonMatt.DesktopIdleStatus.KWinBridge1";
pub const POLICY_IFACE: &str = "org.kde.Solid.PowerManagement.PolicyAgent";
pub const POLICY_PATH: &str = "/org/kde/Solid/PowerManagement/PolicyAgent";
pub const POWER_IFACE: &str = "org.kde.Solid.PowerManagement";
pub const POWER_PATH: &str = "/org/kde/Solid/PowerManagement";
pub async fn current_profile(conn: &Connection) -> Result<crate::config::Profile, String> {
    let p = proxy(conn, POWER_IFACE, POWER_PATH, POWER_IFACE)
        .await
        .map_err(|e| e.to_string())?;
    let id: String = p
        .call("currentProfile", &())
        .await
        .map_err(|e| e.to_string())?;
    crate::config::Profile::from_id(&id).ok_or_else(|| format!("Unknown PowerDevil profile: {id}"))
}
pub const SAVER_NAME: &str = "org.kde.PlasmaVisualScreensaver";
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub async fn proxy<'a, D>(
    conn: &Connection,
    dest: D,

    path: &'a str,
    iface: &'a str,
) -> zbus::Result<Proxy<'a>>
where
    D: TryInto<zbus::names::BusName<'a>>,
    D::Error: Into<zbus::Error>,
{
    zbus::proxy::Builder::<Proxy<'a>>::new(conn)
        .destination(dest)?
        .path(path)?
        .interface(iface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}
fn string(row: &Row, key: &str) -> String {
    row.get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or_default()
        .into()
}
fn boolean(row: &Row, key: &str) -> bool {
    row.get(key)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}
#[derive(Debug)]
pub struct BridgeSnapshot {
    pub revision: u64,
    pub blockers: Vec<Blocker>,
}
#[derive(Debug)]
pub struct BridgeFailure {
    pub code: &'static str,
    pub reason: String,
}
impl BridgeFailure {
    fn new(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
    pub fn error(reason: impl Into<String>) -> Self {
        Self::new("bridge-error", reason)
    }
}
async fn kwin_owner(conn: &Connection) -> Result<String, BridgeFailure> {
    let bus = proxy(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    .map_err(|e| BridgeFailure::error(e.to_string()))?;
    bus.call("GetNameOwner", &("org.kde.KWin",))
        .await
        .map_err(|e| BridgeFailure::error(e.to_string()))
}
async fn ensure_kwin_owner(conn: &Connection, owner: &str) -> Result<(), BridgeFailure> {
    if kwin_owner(conn).await? != owner {
        return Err(BridgeFailure::error(
            "KWin owner changed during bridge request",
        ));
    }
    Ok(())
}
async fn compatible_bridge(conn: &Connection) -> Result<(String, Proxy<'_>), BridgeFailure> {
    let owner = kwin_owner(conn).await?;
    let p = proxy(conn, owner.clone(), "/DesktopIdleStatus", BRIDGE_IFACE)
        .await
        .map_err(|e| BridgeFailure::error(e.to_string()))?;
    let version = p
        .get_property::<u32>("InterfaceVersion")
        .await
        .map_err(|e| {
            BridgeFailure::new(
                "bridge-missing",
                format!("KWin bridge missing or not loaded: {e}"),
            )
        })?;
    if version != 1 {
        return Err(BridgeFailure::new(
            "bridge-version-mismatch",
            format!("KWin bridge interface version {version}; expected 1"),
        ));
    }
    let built: String = p
        .get_property("BuiltForKWin")
        .await
        .map_err(|e| BridgeFailure::error(format!("KWin bridge metadata unavailable: {e}")))?;
    let kwin = proxy(conn, owner.clone(), "/KWin", "org.kde.KWin")
        .await
        .map_err(|e| BridgeFailure::error(e.to_string()))?;
    let support: String = kwin
        .call("supportInformation", &())
        .await
        .map_err(|e| BridgeFailure::error(format!("Running KWin version unavailable: {e}")))?;
    let running = support
        .lines()
        .find_map(|line| line.trim().strip_prefix("KWin version:").map(str::trim))
        .ok_or_else(|| BridgeFailure::error("Running KWin version unavailable"))?;
    if running != built {
        return Err(BridgeFailure::new(
            "bridge-version-mismatch",
            format!("KWin bridge built for {built}, running {running}"),
        ));
    }
    ensure_kwin_owner(conn, &owner).await?;
    Ok((owner, p))
}
/// Validate metadata immediately before the write, and address the validated unique
/// owner so replacement of the well-known name cannot redirect the action.
pub async fn activate_window(conn: &Connection, internal_id: &str) -> Result<bool, BridgeFailure> {
    let (owner, p) = compatible_bridge(conn).await?;
    let result = p
        .call("ActivateWindow", &(internal_id,))
        .await
        .map_err(|e| BridgeFailure::error(e.to_string()))?;
    ensure_kwin_owner(conn, &owner).await?;
    Ok(result)
}
pub async fn bridge(
    conn: &Connection,
    index: &DesktopIndex,
    previous: &[Blocker],
) -> Result<BridgeSnapshot, BridgeFailure> {
    let (owner, p) = compatible_bridge(conn).await?;
    let (revision, rows): (u64, Vec<Row>) = p
        .call("Snapshot", &())
        .await
        .map_err(|e| BridgeFailure::error(format!("KWin bridge snapshot failed: {e}")))?;
    ensure_kwin_owner(conn, &owner).await?;
    let mut blockers = Vec::new();
    for row in rows {
        if !boolean(&row, "effective") {
            continue;
        }
        let app_id = string(&row, "appId");
        let desktop = string(&row, "desktopFileName");
        let executable = string(&row, "executablePath");
        let class = string(&row, "resourceClass");
        if [
            app_id.as_str(),
            desktop.as_str(),
            class.as_str(),
            executable.as_str(),
        ]
        .iter()
        .any(|v| is_screensaver(v, ""))
        {
            continue;
        }
        let (app_name, icon_name) = index.resolve(&desktop, &app_id, &class, &executable);
        let app_id = if !app_id.is_empty() {
            app_id
        } else if !desktop.is_empty() {
            desktop.trim_end_matches(".desktop").into()
        } else {
            class
        };
        let id = string(&row, "internalId");
        if id.is_empty() {
            continue;
        }
        let since = previous
            .iter()
            .find(|b| b.internal_id == id)
            .map_or_else(now, |b| b.since);
        let caption = crate::identity::strip_title(&string(&row, "caption"), &app_name);
        blockers.push(Blocker {
            internal_id: id,
            app_id,
            app_name,
            icon_name,
            caption,
            since,
        });
    }
    blockers.sort_by(|a, b| {
        (&a.since, &a.app_name, &a.caption, &a.internal_id).cmp(&(
            &b.since,
            &b.app_name,
            &b.caption,
            &b.internal_id,
        ))
    });
    Ok(BridgeSnapshot { revision, blockers })
}
pub async fn policies(conn: &Connection) -> zbus::Result<(Vec<Policy>, Vec<Policy>)> {
    Ok((
        requested_policies(conn).await?,
        active_policies(conn).await?,
    ))
}
pub async fn requested_policies(conn: &Connection) -> zbus::Result<Vec<Policy>> {
    policy_property(conn, "RequestedInhibitions").await
}
pub async fn active_policies(conn: &Connection) -> zbus::Result<Vec<Policy>> {
    policy_property(conn, "ActiveInhibitions").await
}
async fn policy_property(conn: &Connection, property: &str) -> zbus::Result<Vec<Policy>> {
    let p = proxy(
        conn,
        "org.kde.Solid.PowerManagement",
        POLICY_PATH,
        POLICY_IFACE,
    )
    .await?;
    type Tuples = Vec<(String, String, String, String, u32)>;
    let rows: Tuples = p.get_property(property).await?;
    Ok(rows
        .into_iter()
        .map(|(what, who, reason, mode, flags)| Policy {
            what,
            who,
            reason,
            mode,
            flags,
        })
        .collect())
}
pub async fn logind(conn: &Connection) -> zbus::Result<Vec<Policy>> {
    let p = proxy(
        conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let rows: Vec<(String, String, String, String, u32, u32)> =
        p.call("ListInhibitors", &()).await?;
    Ok(rows
        .into_iter()
        .map(|(what, who, reason, mode, _, _)| Policy {
            what,
            who,
            reason,
            mode,
            flags: 1,
        })
        .collect())
}
pub async fn saver_present(conn: &Connection) -> zbus::Result<bool> {
    let p = proxy(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    p.call("NameHasOwner", &(SAVER_NAME,)).await
}
pub fn saver_showing(requested: &[Policy]) -> bool {
    requested
        .iter()
        .any(|p| p.reason == "Visual screensaver is active" && is_screensaver(&p.who, &p.reason))
}
pub async fn can_suspend(conn: &Connection) -> zbus::Result<bool> {
    let p = proxy(
        conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let can: String = p.call("CanSuspend", &()).await?;
    Ok(matches!(can.as_str(), "yes" | "challenge"))
}
pub async fn is_vm(conn: &Connection) -> zbus::Result<bool> {
    let p = proxy(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;
    let vm: String = p.get_property("Virtualization").await?;
    Ok(!vm.is_empty())
}
pub async fn bounded<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .map_err(|_| "Desktop D-Bus query timed out".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn own_portal_and_fallback_are_running_hints() {
        let mut p = Policy {
            what: "idle".into(),
            who: String::new(),
            reason: "Visual screensaver is active".into(),
            mode: "block".into(),
            flags: 3,
        };
        assert!(saver_showing(&[p.clone()]));
        p.who = "some-other-app".into();
        assert!(!saver_showing(&[p]));
    }
}
