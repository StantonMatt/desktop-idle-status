// Test-only probe loaded into plasmoidviewer. Uses its real AppletQuickItem.
#include <QCoreApplication>
#include <QGuiApplication>
#include <QQuickWindow>
#include <QQuickItem>
#include <QQuickItemGrabResult>
#include <QTimer>
#include <QFile>
#include <QLocale>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCall>
#include <QPainter>
#include <cstdlib>
#include <QTextStream>
#include <QSharedPointer>
#include <QJSValue>

static QQuickItem *root = nullptr;
static int attempts = 0;
static QObject *findNamed(QQuickItem *item, const QString &name) {
    if (item->objectName() == name) return item;
    if (auto *object = item->findChild<QObject *>(name)) return object;
    for (auto *child : item->childItems()) if (auto *found = findNamed(child,name)) return found;
    return nullptr;
}
static void saveItem(QQuickItem *item, const QString &suffix, std::function<void()> next) {
    if (!item) { qFatal("Missing %s representation", qPrintable(suffix)); }
    auto grab = item->grabToImage();
    if (!grab) qFatal("Cannot grab %s", qPrintable(suffix));
    QObject::connect(grab.data(), &QQuickItemGrabResult::ready, item, [grab, suffix, next]() {
        const QString file = qEnvironmentVariable("DIS_SHOT_PREFIX") + "-" + suffix + ".png";
        QImage screenshot = grab->image();
        QImage background(screenshot.size(), QImage::Format_ARGB32_Premultiplied);
        background.fill(QColor(qEnvironmentVariable("DIS_THEME") == "dark" ? "#2a2e32" : "#eff0f1"));
        QPainter painter(&background); painter.drawImage(0, 0, screenshot); painter.end();
        if (!background.save(file)) qFatal("Cannot save screenshot");
        qInfo("Saved %s", qPrintable(file));
        next();
    });
}
static QQuickItem *findItem(QQuickItem *item) {
    if (item->property("iconState").isValid()) return item;
    for (auto *child : item->childItems()) if (auto *found = findItem(child)) return found;
    return nullptr;
}
static void findRoot() {
    for (auto *window : QGuiApplication::allWindows()) {
        auto *quick = qobject_cast<QQuickWindow *>(window);
        if (!quick) continue;
        root = findItem(quick->contentItem());
        if (root) break;
    }
    if (!root) {
        if (++attempts > 30) qFatal("Applet root not found");
        QTimer::singleShot(100, findRoot); return;
    }
    qInfo() << "Locale" << QLocale::system().name() << QLocale().name() << qEnvironmentVariable("LC_TIME") << root->property("timeLocale");
    if (root->property("toolTipTextFormat").toInt() != Qt::PlainText) qFatal("Tray tooltip is not plain text");
    if (qEnvironmentVariable("DIS_TEST_STATE") == "retention") {
        const auto history = root->property("history").value<QJSValue>();
        if (history.property("length").toInt() != 1) qFatal("Missing retained history row");
        root->setProperty("now", root->property("now").toDouble() + 61);
        if (root->property("history").value<QJSValue>().property("length").toInt() != 0)
            qFatal("History did not expire when the clock advanced");
        qInfo("RETENTION expired on clock advance");
    }
    root->setProperty("preferredRepresentation", root->property("compactRepresentation"));
    root->setProperty("expanded", false);
    QTimer::singleShot(300, [] {
        auto *compact = root->property("compactRepresentationItem").value<QQuickItem *>();
        saveItem(compact, "compact", [] {
            root->setProperty("expanded", true);
            QTimer::singleShot(500, [] {
                auto *full = root->property("fullRepresentationItem").value<QQuickItem *>();
                if (qEnvironmentVariable("DIS_TEST_STATE") == "history-error") {
                    auto *status = findNamed(full, "historyStatus");
                    if (!status || !status->property("visible").toBool() || status->property("text").toString() != "Couldn't read history")
                        qFatal("History error was presented as empty");
                    qInfo("HISTORY_ERROR visible");
                }
                if (qEnvironmentVariable("DIS_TEST_STATE") == "late-power-lock") {
                    auto *row = findNamed(full, "setting-lock");
                    if (!row || row->property("caption").toString() != "Power Management"
                        || row->property("iconName").toString() != "preferences-system-power-management")
                        qFatal("PowerDevil lock points to the wrong settings page");
                    qInfo("POWER_LOCK correct settings page");
                }
                if (qEnvironmentVariable("DIS_TEST_STATE") == "late-seconds") {
                    auto *row = findNamed(full, "setting-lock");
                    if (!row || row->property("time").toString() != "After 20 s")
                        qFatal("Subminute deadline lost precision");
                }
                saveItem(full, "full", [] {
                    QFile output(qEnvironmentVariable("DIS_SHOT_PREFIX") + "-state.json");
                    if (!output.open(QIODevice::WriteOnly)) qFatal("Cannot save state");
                    QTextStream stream(&output);
                    stream << root->property("state").toString() << "\n" << root->property("toolTipMainText").toString() << "\n" << root->property("toolTipSubText").toString() << "\n";
                    stream << root->property("statusSubtext").toString() << "\n";
                    output.close();
                    if (qEnvironmentVariable("DIS_EXERCISE") == "1") {
                        auto *full = root->property("fullRepresentationItem").value<QQuickItem *>();
                        auto click = [full](const QString &name) {
                            auto *button = findNamed(full,name);
                            if (!button) qFatal("Missing control %s", qPrintable(name));
                            if (!QMetaObject::invokeMethod(button, "click")) qFatal("Cannot click control");
                            qInfo("CLICK %s", qPrintable(name));
                        };
                        if (qEnvironmentVariable("DIS_EMIT_RETURN") != "0") {
                            const auto state = root->property("state").toString();
                            if (state == "blocked") {
                                if (findNamed(full,"blocker-firefox-id")) click("blocker-firefox-id");
                                else click("blocker-unattributed"); // Must remain inert.
                            }
                            if (state == "late") {
                                click("setting-lock");
                                click("setting-dim");
                            }
                            if (state == "screensaver-off") click("startScreensaverButton");
                            if (state == "service-down") {
                                qInfo("TRIGGER startServiceAction");
                                auto *action = findNamed(full,"startServiceAction");
                                if (!action || !QMetaObject::invokeMethod(action,"trigger")) qFatal("Cannot start service");
                            }
                            if (state != "service-down" && state != "loading") {
                                auto *clear = findNamed(root,"clearHistoryAction");
                                if (!clear || !QMetaObject::invokeMethod(clear,"trigger")) qFatal("Cannot clear history");
                            }
                            root->setProperty("expanded", false);
                            QDBusConnection::sessionBus().asyncCall(QDBusMessage::createMethodCall("org.example.Fixture", "/Fixture", "org.example.Fixture", "EmitReturn"));
                        }
                        QTimer::singleShot(1800, [] {
                            if (qEnvironmentVariable("DIS_TEST_STATE") == "clear-error") {
                                auto *full = root->property("fullRepresentationItem").value<QQuickItem *>();
                                auto *status = findNamed(full, "historyStatus");
                                if (!status || status->property("text").toString() != "Couldn't clear history")
                                    qFatal("Clear failure was ignored");
                                qInfo("CLEAR_ERROR visible");
                            }
                            qInfo() << "NOTIFICATION_ACTION expanded=" << root->property("expanded");
                            std::_Exit(0);
                        });
                    } else std::_Exit(0);
                });
            });
        });
    });
}
static void startup() { QTimer::singleShot(1800, findRoot); }
Q_COREAPP_STARTUP_FUNCTION(startup)
