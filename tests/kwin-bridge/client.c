// SPDX-License-Identifier: GPL-3.0-or-later
// A tiny mapped xdg toplevel; newline-delimited commands, no D-Bus.
// m=minimize, r/i=release/create parent inhibitor, j/k=second object,
// s=subsurface-only inhibition, u=release subsurface inhibitor,
// t TITLE/d APP_ID=identity, a=activation, p=roundtrip ack, q=exit.
#define _POSIX_C_SOURCE 200809L
#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"
#include "idle-inhibit-unstable-v1-client-protocol.h"
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

struct client {
    struct wl_display *display;
    struct wl_compositor *compositor;
    struct wl_subcompositor *subcompositor;
    struct wl_shm *shm;
    struct xdg_wm_base *shell;
    struct zwp_idle_inhibit_manager_v1 *manager;
    struct wl_surface *surface;
    struct xdg_surface *xdg;
    struct xdg_toplevel *toplevel;
    struct wl_buffer *buffer;
    struct zwp_idle_inhibitor_v1 *inhibitor;
    struct zwp_idle_inhibitor_v1 *second_inhibitor;
    struct zwp_idle_inhibitor_v1 *sub_inhibitor;
    struct wl_surface *child;
    struct wl_subsurface *subsurface;
    bool configured;
    bool activated;
    bool running;
};

static void ping(void *data, struct xdg_wm_base *shell, uint32_t serial)
{
    (void)data;
    xdg_wm_base_pong(shell, serial);
}
static const struct xdg_wm_base_listener shell_listener = {.ping = ping};

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version)
{
    (void)version;
    struct client *c = data;
    if (strcmp(interface, "wl_compositor") == 0)
        c->compositor = wl_registry_bind(registry, name, &wl_compositor_interface, 4);
    else if (strcmp(interface, "wl_subcompositor") == 0)
        c->subcompositor = wl_registry_bind(registry, name, &wl_subcompositor_interface, 1);
    else if (strcmp(interface, "wl_shm") == 0)
        c->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
    else if (strcmp(interface, "xdg_wm_base") == 0)
        c->shell = wl_registry_bind(registry, name, &xdg_wm_base_interface, 1);
    else if (strcmp(interface, "zwp_idle_inhibit_manager_v1") == 0)
        c->manager = wl_registry_bind(registry, name, &zwp_idle_inhibit_manager_v1_interface, 1);
}
static void global_remove(void *data, struct wl_registry *registry, uint32_t name)
{
    (void)data; (void)registry; (void)name;
}
static const struct wl_registry_listener registry_listener = {
    .global = global, .global_remove = global_remove,
};

static void configure(void *data, struct xdg_surface *surface, uint32_t serial)
{
    struct client *c = data;
    xdg_surface_ack_configure(surface, serial);
    wl_surface_attach(c->surface, c->buffer, 0, 0);
    wl_surface_damage(c->surface, 0, 0, 320, 200);
    wl_surface_commit(c->surface);
    if (!c->configured) {
        c->configured = true;
        puts("mapped");
        fflush(stdout);
    }
}
static const struct xdg_surface_listener surface_listener = {.configure = configure};
static void toplevel_configure(void *data, struct xdg_toplevel *toplevel,
                               int32_t width, int32_t height, struct wl_array *states)
{
    (void)toplevel; (void)width; (void)height;
    struct client *c = data;
    c->activated = false;
    uint32_t *state;
    wl_array_for_each(state, states) {
        if (*state == XDG_TOPLEVEL_STATE_ACTIVATED) c->activated = true;
    }
}
static void close_window(void *data, struct xdg_toplevel *toplevel)
{
    (void)toplevel;
    ((struct client *)data)->running = false;
}
static const struct xdg_toplevel_listener toplevel_listener = {
    .configure = toplevel_configure, .close = close_window,
};

static struct wl_buffer *make_buffer(struct wl_shm *shm)
{
    const size_t size = 320 * 200 * 4;
    const char *runtime = getenv("XDG_RUNTIME_DIR");
    if (!runtime) return NULL;
    char *path = malloc(strlen(runtime) + 32);
    if (!path) return NULL;
    sprintf(path, "%s/dis-buffer-XXXXXX", runtime);
    int fd = mkstemp(path);
    unlink(path);
    free(path);
    if (fd < 0) return NULL;
    if (ftruncate(fd, (off_t)size) < 0) { close(fd); return NULL; }
    uint32_t *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (pixels == MAP_FAILED) { close(fd); return NULL; }
    for (size_t i = 0; i < size / 4; ++i) pixels[i] = 0xff345678;
    munmap(pixels, size);
    struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, (int32_t)size);
    struct wl_buffer *buffer = wl_shm_pool_create_buffer(pool, 0, 320, 200, 320 * 4, WL_SHM_FORMAT_XRGB8888);
    wl_shm_pool_destroy(pool);
    close(fd);
    return buffer;
}

static void command(struct client *c, const char *line)
{
    if (strcmp(line, "m") == 0) xdg_toplevel_set_minimized(c->toplevel);
    if (strcmp(line, "r") == 0 && c->inhibitor) {
        zwp_idle_inhibitor_v1_destroy(c->inhibitor); c->inhibitor = NULL;
    }
    if (strcmp(line, "i") == 0 && !c->inhibitor)
        c->inhibitor = zwp_idle_inhibit_manager_v1_create_inhibitor(c->manager, c->surface);
    if (strcmp(line, "j") == 0 && !c->second_inhibitor)
        c->second_inhibitor = zwp_idle_inhibit_manager_v1_create_inhibitor(c->manager, c->surface);
    if (strcmp(line, "k") == 0 && c->second_inhibitor) {
        zwp_idle_inhibitor_v1_destroy(c->second_inhibitor); c->second_inhibitor = NULL;
    }
    if (strcmp(line, "s") == 0) {
        if (!c->child) {
            c->child = wl_compositor_create_surface(c->compositor);
            c->subsurface = wl_subcompositor_get_subsurface(c->subcompositor, c->child, c->surface);
            wl_subsurface_set_desync(c->subsurface);
            wl_surface_attach(c->child, c->buffer, 0, 0);
            wl_surface_damage(c->child, 0, 0, 320, 200);
            wl_surface_commit(c->child);
            wl_surface_commit(c->surface);
        }
        if (!c->sub_inhibitor)
            c->sub_inhibitor = zwp_idle_inhibit_manager_v1_create_inhibitor(c->manager, c->child);
        command(c, "r");
    }
    if (strcmp(line, "u") == 0 && c->sub_inhibitor) {
        zwp_idle_inhibitor_v1_destroy(c->sub_inhibitor); c->sub_inhibitor = NULL;
    }
    if (strncmp(line, "t ", 2) == 0) xdg_toplevel_set_title(c->toplevel, line + 2);
    if (strncmp(line, "d ", 2) == 0) xdg_toplevel_set_app_id(c->toplevel, line + 2);
    if (strcmp(line, "q") == 0) c->running = false;
    if (strcmp(line, "a") == 0) {
        printf("activated=%d\n", c->activated);
        fflush(stdout);
    }
    if (strcmp(line, "p") == 0) {
        if (wl_display_roundtrip(c->display) < 0) c->running = false;
        puts("ack");
        fflush(stdout);
    }
}

int main(int argc, char **argv)
{
    if (argc != 2) { fprintf(stderr, "usage: %s CAPTION\n", argv[0]); return 2; }
    struct client c = {.running = true};
    c.display = wl_display_connect(NULL);
    if (!c.display) { perror("wl_display_connect"); return 1; }
    struct wl_registry *registry = wl_display_get_registry(c.display);
    wl_registry_add_listener(registry, &registry_listener, &c);
    if (wl_display_roundtrip(c.display) < 0 || !c.compositor || !c.subcompositor || !c.shm || !c.shell || !c.manager) {
        fprintf(stderr, "required Wayland globals missing\n"); return 1;
    }
    xdg_wm_base_add_listener(c.shell, &shell_listener, &c);
    c.buffer = make_buffer(c.shm);
    if (!c.buffer) { perror("buffer"); return 1; }
    c.surface = wl_compositor_create_surface(c.compositor);
    c.xdg = xdg_wm_base_get_xdg_surface(c.shell, c.surface);
    xdg_surface_add_listener(c.xdg, &surface_listener, &c);
    c.toplevel = xdg_surface_get_toplevel(c.xdg);
    xdg_toplevel_add_listener(c.toplevel, &toplevel_listener, &c);
    xdg_toplevel_set_title(c.toplevel, argv[1]);
    xdg_toplevel_set_app_id(c.toplevel, "io.github.StantonMatt.DesktopIdleStatus.Test");
    c.inhibitor = zwp_idle_inhibit_manager_v1_create_inhibitor(c.manager, c.surface);
    wl_surface_commit(c.surface);
    char line[1024];
    size_t used = 0;
    while (c.running) {
        while (wl_display_prepare_read(c.display) != 0) {
            if (wl_display_dispatch_pending(c.display) < 0) goto failed;
        }
        int flushed = wl_display_flush(c.display);
        if (flushed < 0 && errno != EAGAIN) {
            wl_display_cancel_read(c.display); goto failed;
        }
        struct pollfd fds[] = {
            {wl_display_get_fd(c.display), POLLIN | (flushed < 0 ? POLLOUT : 0), 0},
            {STDIN_FILENO, POLLIN, 0},
        };
        if (poll(fds, 2, -1) < 0) {
            wl_display_cancel_read(c.display);
            if (errno == EINTR) continue;
            goto failed;
        }
        if (fds[0].revents & POLLIN) {
            if (wl_display_read_events(c.display) < 0) goto failed;
        } else {
            wl_display_cancel_read(c.display);
        }
        if (wl_display_dispatch_pending(c.display) < 0) goto failed;
        if (fds[0].revents & (POLLERR | POLLHUP | POLLNVAL)) goto failed;
        if (fds[1].revents & (POLLIN | POLLHUP)) {
            char bytes[256];
            ssize_t count = read(STDIN_FILENO, bytes, sizeof(bytes));
            if (count <= 0) c.running = false;
            for (ssize_t i = 0; i < count; ++i) {
                if (bytes[i] == '\n') {
                    line[used] = '\0';
                    command(&c, line);
                    used = 0;
                } else if (used + 1 < sizeof(line)) {
                    line[used++] = bytes[i];
                } else {
                    fprintf(stderr, "command too long\n");
                    goto failed;
                }
            }
        }
    }
    if (c.sub_inhibitor) zwp_idle_inhibitor_v1_destroy(c.sub_inhibitor);
    if (c.second_inhibitor) zwp_idle_inhibitor_v1_destroy(c.second_inhibitor);
    if (c.inhibitor) zwp_idle_inhibitor_v1_destroy(c.inhibitor);
    if (c.subsurface) wl_subsurface_destroy(c.subsurface);
    if (c.child) wl_surface_destroy(c.child);
    xdg_toplevel_destroy(c.toplevel);
    xdg_surface_destroy(c.xdg);
    wl_surface_destroy(c.surface);
    wl_buffer_destroy(c.buffer);
    zwp_idle_inhibit_manager_v1_destroy(c.manager);
    xdg_wm_base_destroy(c.shell);
    wl_shm_destroy(c.shm);
    wl_subcompositor_destroy(c.subcompositor);
    wl_compositor_destroy(c.compositor);
    wl_registry_destroy(registry);
    wl_display_flush(c.display);
    wl_display_disconnect(c.display);
    return 0;
failed:
    fprintf(stderr, "Wayland connection failed\n");
    wl_display_disconnect(c.display);
    return 1;
}
