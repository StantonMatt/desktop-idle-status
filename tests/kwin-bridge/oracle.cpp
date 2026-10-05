// SPDX-License-Identifier: GPL-2.0-or-later
// Test-only oracle. Never installed or loaded outside run.sh's private KWin.
#include <plugin.h>
#include <input.h>
#include <window.h>
#include <workspace.h>
#include <QDBusConnection>
#include <QPointer>

class Oracle : public KWin::Plugin
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "io.github.StantonMatt.DesktopIdleStatus.TestOracle")
public:
    Oracle() { QDBusConnection::sessionBus().registerObject("/IdleTestOracle", this, QDBusConnection::ExportAllSlots); }
    ~Oracle() override { QDBusConnection::sessionBus().unregisterObject("/IdleTestOracle"); }
public Q_SLOTS:
    QStringList EffectiveWindows() const
    {
        QStringList ids;
        for (auto *window : KWin::input()->idleInhibitors()) {
            ids.append(window->internalId().toString(QUuid::WithoutBraces));
        }
        return ids;
    }
    bool SetHidden(const QString &id, bool hidden)
    {
        QPointer<KWin::Window> window = KWin::workspace()->findWindow(QUuid::fromString(id));
        if (!window) return false;
        window->setHidden(hidden);
        return true;
    }
    void SetShowingDesktop(bool showing) { KWin::workspace()->setShowingDesktop(showing, false); }
    bool ShowingDesktop() const { return KWin::workspace()->showingDesktop(); }
};
class OracleFactory : public KWin::PluginFactory
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID PluginFactory_iid FILE "oracle.json")
    Q_INTERFACES(KWin::PluginFactory)
public:
    std::unique_ptr<KWin::Plugin> create() const override { return std::make_unique<Oracle>(); }
};
#include "oracle.moc"
