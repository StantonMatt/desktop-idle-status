// SPDX-License-Identifier: GPL-2.0-or-later
#include "bridge.h"

class DesktopIdleStatusFactory : public KWin::PluginFactory
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID PluginFactory_iid FILE "metadata.json")
    Q_INTERFACES(KWin::PluginFactory)
public:
    std::unique_ptr<KWin::Plugin> create() const override
    {
        return std::make_unique<DesktopIdleStatus::Bridge>();
    }
};

#include "main.moc"
