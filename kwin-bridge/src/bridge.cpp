// SPDX-License-Identifier: GPL-2.0-or-later
#include "bridge.h"
#include "build-info.h"

#include <input.h>
#include <main.h>
#include <virtualdesktops.h>
#include <wayland/clientconnection.h>
#include <wayland/surface.h>
#include <window.h>
#include <workspace.h>
#include <QDBusConnection>
#include <QDBusMetaType>
#include <QDebug>
#include <algorithm>

namespace DesktopIdleStatus {
static const QString objectPath = QStringLiteral("/DesktopIdleStatus");

Bridge::Bridge()
{
    qRegisterMetaType<WindowRows>("DesktopIdleStatus::WindowRows");
    qDBusRegisterMetaType<WindowRows>();
    m_coalesce.setSingleShot(true);
    m_coalesce.setInterval(0);
    connect(&m_coalesce, &QTimer::timeout, this, &Bridge::refresh);
    m_poll.setInterval(1000);
    connect(&m_poll, &QTimer::timeout, this, &Bridge::refresh);
    connect(KWin::kwinApp(), &KWin::Application::workspaceCreated, this, &Bridge::attachWorkspace);
    attachWorkspace();
    m_registered = QDBusConnection::sessionBus().registerObject(
        objectPath, this, QDBusConnection::ExportAllSlots | QDBusConnection::ExportAllSignals | QDBusConnection::ExportAllProperties);
    if (!m_registered) {
        qWarning() << "Desktop Idle Status: could not register D-Bus object";
    }
    m_poll.start();
}

Bridge::~Bridge()
{
    if (m_registered) {
        QDBusConnection::sessionBus().unregisterObject(objectPath);
    }
}

QString Bridge::builtForKWin() const { return QStringLiteral(KWIN_PLUGIN_VERSION_STRING); }
QString Bridge::builtAgainstPackage() const { return QStringLiteral(DESKTOP_IDLE_STATUS_KWIN_PACKAGE); }

void Bridge::schedule()
{
    if (!m_coalesce.isActive()) {
        m_coalesce.start();
    }
}

void Bridge::attachWorkspace()
{
    auto *ws = KWin::workspace();
    if (!ws || m_workspaceAttached) {
        return;
    }
    m_workspaceAttached = true;
    connect(ws, &KWin::Workspace::windowAdded, this, [this](KWin::Window *window) {
        watchWindow(window);
        schedule();
    });
    connect(ws, &KWin::Workspace::windowRemoved, this, &Bridge::schedule);
    connect(ws, &KWin::Workspace::currentDesktopChanged, this, &Bridge::schedule);
    for (auto *window : ws->windows()) {
        watchWindow(window);
    }
    refresh();
}

void Bridge::watchWindow(KWin::Window *window)
{
    auto it = std::find_if(m_watches.begin(), m_watches.end(), [window](const Watch &watch) {
        return watch.window == window;
    });
    if (it == m_watches.end()) {
        m_watches.append({window, nullptr});
        it = std::prev(m_watches.end());
        connect(window, &KWin::Window::desktopsChanged, this, &Bridge::schedule);
        connect(window, &KWin::Window::minimizedChanged, this, &Bridge::schedule);
        connect(window, &KWin::Window::hiddenChanged, this, &Bridge::schedule);
        connect(window, &KWin::Window::closed, this, &Bridge::schedule);
        connect(window, &KWin::Window::captionChanged, this, &Bridge::schedule);
        connect(window, &KWin::Window::windowClassChanged, this, &Bridge::schedule);
        connect(window, &KWin::Window::desktopFileNameChanged, this, &Bridge::schedule);
    }
    // A surface can be assigned after windowAdded; the safety poll catches it.
    if (window->surface() && it->surface != window->surface()) {
        it->surface = window->surface();
        connect(window->surface(), &KWin::SurfaceInterface::inhibitsIdleChanged, this, &Bridge::schedule);
    }
}

WindowRows Bridge::collect() const
{
    WindowRows rows;
    if (!KWin::workspace() || !KWin::input()) {
        return rows;
    }
    const auto effective = KWin::input()->idleInhibitors();
    auto candidates = KWin::workspace()->windows();
    for (auto *window : effective) {
        if (!candidates.contains(window)) {
            candidates.append(window);
        }
    }
    for (auto *window : candidates) {
        auto *surface = window->surface();
        const bool active = effective.contains(window);
        if (!active && (window->isDeleted() || window->isInternal() || window->isUnmanaged()
                        || !surface || !surface->inhibitsIdle())) {
            continue;
        }
        QString reason;
        if (!active) {
            if (window->isMinimized()) {
                reason = QStringLiteral("minimized");
            } else if (!window->isOnCurrentDesktop()) {
                reason = QStringLiteral("other-desktop");
            } else {
                reason = QStringLiteral("hidden");
            }
        }
        auto *client = surface ? surface->client() : nullptr;
        rows.append({
            {QStringLiteral("internalId"), window->internalId().toString(QUuid::WithoutBraces)},
            {QStringLiteral("pid"), uint(std::max<pid_t>(0, window->pid()))},
            {QStringLiteral("executablePath"), client ? client->executablePath() : QString()},
            {QStringLiteral("appId"), client ? client->securityContextAppId() : QString()},
            {QStringLiteral("desktopFileName"), window->desktopFileName()},
            {QStringLiteral("resourceClass"), window->resourceClass()},
            {QStringLiteral("caption"), window->caption()},
            {QStringLiteral("effective"), active},
            {QStringLiteral("notEffectiveReason"), reason},
        });
    }
    std::sort(rows.begin(), rows.end(), [](const QVariantMap &a, const QVariantMap &b) {
        return a.value(QStringLiteral("internalId")).toString() < b.value(QStringLiteral("internalId")).toString();
    });
    return rows;
}

void Bridge::refresh()
{
    m_watches.removeIf([](const Watch &watch) { return watch.window.isNull(); });
    if (KWin::workspace()) {
        for (auto *window : KWin::workspace()->windows()) {
            watchWindow(window);
        }
    }
    const auto next = collect();
    if (next != m_rows) {
        m_rows = next;
        Q_EMIT Changed(++m_revision);
    }
}

qulonglong Bridge::Snapshot(WindowRows &windows)
{
    // Flush pending changes so the reply and revision describe the same state.
    refresh();
    windows = m_rows;
    return m_revision;
}

bool Bridge::ActivateWindow(const QString &internalId)
{
    if (!KWin::workspace()) {
        return false;
    }
    const auto id = QUuid::fromString(internalId);
    if (id.isNull()) {
        return false;
    }
    QPointer<KWin::Window> window = KWin::workspace()->findWindow(id);
    if (!window || !window->isClient() || window->isDeleted()) {
        return false;
    }
    if (!window->isOnCurrentDesktop() && !window->desktops().isEmpty()) {
        KWin::VirtualDesktopManager::self()->setCurrent(window->desktops().first());
    }
    if (!window) {
        return false;
    }
    window->setMinimized(false);
    if (!window) {
        return false;
    }
    KWin::workspace()->activateWindow(window);
    schedule();
    return true;
}
}
