// SPDX-License-Identifier: GPL-2.0-or-later
#pragma once

#include <plugin.h>
#include <QPointer>
#include <QTimer>
#include <QVariantMap>

namespace KWin {
class Window;
class SurfaceInterface;
}

namespace DesktopIdleStatus {
using WindowRows = QList<QVariantMap>;

class Bridge : public KWin::Plugin
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "io.github.StantonMatt.DesktopIdleStatus.KWinBridge1")
    Q_PROPERTY(uint InterfaceVersion READ interfaceVersion CONSTANT)
    Q_PROPERTY(QString BuiltForKWin READ builtForKWin CONSTANT)
    Q_PROPERTY(QString BuiltAgainstPackage READ builtAgainstPackage CONSTANT)
public:
    Bridge();
    ~Bridge() override;
    uint interfaceVersion() const { return 1; }
    QString builtForKWin() const;
    QString builtAgainstPackage() const;

public Q_SLOTS:
    qulonglong Snapshot(DesktopIdleStatus::WindowRows &windows);
    bool ActivateWindow(const QString &internalId);

Q_SIGNALS:
    void Changed(qulonglong revision);

private:
    struct Watch {
        QPointer<KWin::Window> window;
        QPointer<KWin::SurfaceInterface> surface;
    };
    void attachWorkspace();
    void watchWindow(KWin::Window *window);
    void refresh();
    void schedule();
    WindowRows collect() const;
    QList<Watch> m_watches;
    WindowRows m_rows;
    qulonglong m_revision = 0;
    QTimer m_coalesce;
    QTimer m_poll;
    bool m_registered = false;
    bool m_workspaceAttached = false;
};
}
Q_DECLARE_METATYPE(DesktopIdleStatus::WindowRows)
