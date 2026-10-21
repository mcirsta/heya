// Synthetic Qt sender, used only on ui-offscreen.sh's private KWin compositor.
// Qt is a test tool here, not a Heya dependency.
#include <QApplication>
#include <QDrag>
#include <QMimeData>
#include <QMouseEvent>
#include <QTimer>
#include <QUrl>
#include <QWidget>
#include <wayland-client.h>
#include <linux/input-event-codes.h>
#include <cstring>
#include <cstdio>
#include "fake-input-client.h"

static org_kde_kwin_fake_input *input = nullptr;
static void global(void *, wl_registry *registry, uint32_t name, const char *interface, uint32_t version) {
    if (!std::strcmp(interface, "org_kde_kwin_fake_input"))
        input = static_cast<org_kde_kwin_fake_input *>(wl_registry_bind(
            registry, name, &org_kde_kwin_fake_input_interface, std::min(version, 4u)));
}
static void removed(void *, wl_registry *, uint32_t) {}
static const wl_registry_listener listener = { global, removed };

class Source : public QWidget {
public:
    QString mode;
    explicit Source(QString mode) : mode(std::move(mode)) { setWindowTitle("Heya drop source"); resize(250, 200); }
    void mousePressEvent(QMouseEvent *event) override {
        if (event->button() != Qt::LeftButton) return;
        auto *mime = new QMimeData;
        if (mode == "text") mime->setText("Not an attachment");
        else mime->setUrls({QUrl::fromLocalFile("/tmp/Sample 你好.png"), QUrl::fromLocalFile("/tmp/Notes.txt")});
        QDrag drag(this);
        drag.setMimeData(mime);
        auto action = drag.exec(Qt::CopyAction);
        std::printf("Native Wayland drag: mode=%s action=%d\n", qPrintable(mode), int(action));
        QApplication::exit((action == Qt::CopyAction) == (mode == "drop") ? 0 : 1);
    }
};
int main(int argc, char **argv) {
    if (argc != 2 || qEnvironmentVariable("HEYA_PRIVATE_DROP_TEST") != "1") return 2;
    QApplication app(argc, argv);
    Source source(QString::fromUtf8(argv[1]));
    auto *display = wl_display_connect(nullptr);
    if (!display) return 3;
    auto *registry = wl_display_get_registry(display);
    wl_registry_add_listener(registry, &listener, nullptr);
    wl_display_roundtrip(display);
    if (!input) return 4;
    org_kde_kwin_fake_input_authenticate(input, "Heya offscreen test", "Synthetic drag on a private test compositor");
    wl_display_roundtrip(display);
    auto move = [display](int x, int y) {
        org_kde_kwin_fake_input_pointer_motion_absolute(input, wl_fixed_from_int(x), wl_fixed_from_int(y));
        wl_display_flush(display);
    };
    source.show();
    QTimer::singleShot(500, &app, [=] { move(100, 100); });
    QTimer::singleShot(800, &app, [=] {
        org_kde_kwin_fake_input_button(input, BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED);
        wl_display_flush(display);
    });
    QTimer::singleShot(1100, &app, [=] { move(900, 400); });
    if (source.mode == "cancel") QTimer::singleShot(1700, &app, [=] { move(1250, 900); });
    QTimer::singleShot(2200, &app, [=] {
        org_kde_kwin_fake_input_button(input, BTN_LEFT, WL_POINTER_BUTTON_STATE_RELEASED);
        wl_display_flush(display);
    });
    QTimer::singleShot(5000, &app, [&] { app.exit(5); });
    int result = app.exec();
    wl_registry_destroy(registry);
    wl_display_disconnect(display);
    return result;
}
