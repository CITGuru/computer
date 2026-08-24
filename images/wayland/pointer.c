// Synthetic pointer input for a Wayland box.
//
// **Wayland gives no client a way to move somebody else's pointer.** The
// compositor owns the seat, and its input devices come from the backend — of
// which a headless one has none, so `sway`'s own `seat cursor` commands are
// accepted and move nothing. Synthetic input has to arrive as a *device*, and
// `zwlr_virtual_pointer_v1` is how a client asks for one. It is the pointer
// half of what `wtype` does for the keyboard.
//
// One gesture per run, and the whole gesture in one run. A virtual pointer
// lives only as long as the client that made it, so a process per event would
// create and destroy a device for each one and race the compositor every time.
//
// It is loud on failure. A compositor that does not offer the protocol, or an
// argument that makes no sense, exits non-zero and says so — the alternative is
// a command that reports success and leaves the screen where it was.

#define _POSIX_C_SOURCE 200809L

#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <wayland-client.h>

// linux/input-event-codes.h, which is what the protocol takes.
#define BTN_LEFT 0x110
#define BTN_RIGHT 0x111
#define BTN_MIDDLE 0x112

// One wheel notch, as a compositor expects to see it.
#define NOTCH 15

static struct wl_display *display;
static struct wl_seat *seat;
static struct wl_output *output;
static struct zwlr_virtual_pointer_manager_v1 *manager;
static struct zwlr_virtual_pointer_v1 *pointer;

// The coordinate space the caller's pixels are against, read from the output
// rather than passed in: a screen that came up at a different size is the one
// the coordinates have to be scaled against, and only the compositor knows it.
static int32_t screen_width;
static int32_t screen_height;

static void registry_global(void *data, struct wl_registry *registry, uint32_t name,
                            const char *interface, uint32_t version) {
	(void)data;
	(void)version;

	if (strcmp(interface, wl_seat_interface.name) == 0) {
		seat = wl_registry_bind(registry, name, &wl_seat_interface, 1);
	} else if (strcmp(interface, zwlr_virtual_pointer_manager_v1_interface.name) == 0) {
		manager = wl_registry_bind(registry, name,
		                           &zwlr_virtual_pointer_manager_v1_interface, 1);
	} else if (strcmp(interface, wl_output_interface.name) == 0 && output == NULL) {
		output = wl_registry_bind(registry, name, &wl_output_interface, 2);
	}
}

static void registry_remove(void *data, struct wl_registry *registry, uint32_t name) {
	(void)data;
	(void)registry;
	(void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = registry_remove,
};

static void output_geometry(void *data, struct wl_output *wl_output, int32_t x, int32_t y,
                            int32_t physical_width, int32_t physical_height, int32_t subpixel,
                            const char *make, const char *model, int32_t transform) {
	(void)data; (void)wl_output; (void)x; (void)y;
	(void)physical_width; (void)physical_height; (void)subpixel;
	(void)make; (void)model; (void)transform;
}

static void output_mode(void *data, struct wl_output *wl_output, uint32_t flags, int32_t width,
                        int32_t height, int32_t refresh) {
	(void)data;
	(void)wl_output;
	(void)refresh;

	if (flags & WL_OUTPUT_MODE_CURRENT) {
		screen_width = width;
		screen_height = height;
	}
}

static void output_done(void *data, struct wl_output *wl_output) {
	(void)data;
	(void)wl_output;
}

static void output_scale(void *data, struct wl_output *wl_output, int32_t factor) {
	(void)data;
	(void)wl_output;
	(void)factor;
}

static const struct wl_output_listener output_listener = {
	.geometry = output_geometry,
	.mode = output_mode,
	.done = output_done,
	.scale = output_scale,
};

static uint32_t now_ms(void) {
	struct timespec at;
	clock_gettime(CLOCK_MONOTONIC, &at);
	return (uint32_t)(at.tv_sec * 1000 + at.tv_nsec / 1000000);
}

static void settle(long ms) {
	struct timespec wait = {.tv_sec = ms / 1000, .tv_nsec = (ms % 1000) * 1000000};
	wl_display_flush(display);
	nanosleep(&wait, NULL);
}

static void move_to(long x, long y) {
	zwlr_virtual_pointer_v1_motion_absolute(pointer, now_ms(), (uint32_t)x, (uint32_t)y,
	                                        (uint32_t)screen_width, (uint32_t)screen_height);
	zwlr_virtual_pointer_v1_frame(pointer);
}

static void button(uint32_t code, uint32_t pressed) {
	zwlr_virtual_pointer_v1_button(pointer, now_ms(), code, pressed);
	zwlr_virtual_pointer_v1_frame(pointer);
}

static void wheel(int down) {
	zwlr_virtual_pointer_v1_axis_source(pointer, WL_POINTER_AXIS_SOURCE_WHEEL);
	zwlr_virtual_pointer_v1_axis_discrete(pointer, now_ms(), WL_POINTER_AXIS_VERTICAL_SCROLL,
	                                      wl_fixed_from_int(down ? NOTCH : -NOTCH),
	                                      down ? 1 : -1);
	zwlr_virtual_pointer_v1_frame(pointer);
}

static uint32_t button_code(const char *name) {
	if (strcmp(name, "left") == 0) {
		return BTN_LEFT;
	}
	if (strcmp(name, "right") == 0) {
		return BTN_RIGHT;
	}
	if (strcmp(name, "middle") == 0) {
		return BTN_MIDDLE;
	}
	fprintf(stderr, "unknown button: %s\n", name);
	exit(2);
}

static long number(const char *text) {
	char *end = NULL;
	long value = strtol(text, &end, 10);
	if (end == text || *end != '\0') {
		fprintf(stderr, "not a number: %s\n", text);
		exit(2);
	}
	return value;
}

static const char *USAGE =
    "usage: computer-pointer move X Y\n"
    "                        click X Y BUTTON\n"
    "                        dblclick X Y BUTTON\n"
    "                        drag X1 Y1 X2 Y2 BUTTON\n"
    "                        scroll X Y NOTCHES   (negative scrolls up)\n";

int main(int argc, char **argv) {
	if (argc < 2) {
		fputs(USAGE, stderr);
		return 2;
	}

	display = wl_display_connect(NULL);
	if (display == NULL) {
		fprintf(stderr, "no compositor on %s in %s\n", getenv("WAYLAND_DISPLAY"),
		        getenv("XDG_RUNTIME_DIR"));
		return 1;
	}

	struct wl_registry *registry = wl_display_get_registry(display);
	wl_registry_add_listener(registry, &registry_listener, NULL);
	wl_display_roundtrip(display);

	if (manager == NULL) {
		fputs("this compositor does not offer zwlr_virtual_pointer_v1\n", stderr);
		return 1;
	}
	if (output == NULL) {
		fputs("this compositor has no output to point at\n", stderr);
		return 1;
	}

	// A second trip, for the mode event the output sends after it is bound.
	wl_output_add_listener(output, &output_listener, NULL);
	wl_display_roundtrip(display);

	if (screen_width <= 0 || screen_height <= 0) {
		fputs("the output never reported a size\n", stderr);
		return 1;
	}

	pointer = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, seat);

	// **Before any event.** The device does not exist until the compositor has
	// made it, and events sent into that gap are dropped with nothing to say
	// they were.
	wl_display_roundtrip(display);

	const char *verb = argv[1];
	int rest = argc - 2;

	if (strcmp(verb, "move") == 0 && rest == 2) {
		move_to(number(argv[2]), number(argv[3]));
	} else if (strcmp(verb, "click") == 0 && rest == 3) {
		uint32_t code = button_code(argv[4]);
		move_to(number(argv[2]), number(argv[3]));
		settle(20);
		button(code, 1);
		button(code, 0);
	} else if (strcmp(verb, "dblclick") == 0 && rest == 3) {
		// One run, not two. Two runs are two devices and two round trips
		// through the runtime, far enough apart that the application sees two
		// single clicks — a different gesture.
		uint32_t code = button_code(argv[4]);
		move_to(number(argv[2]), number(argv[3]));
		settle(20);
		button(code, 1);
		button(code, 0);
		settle(40);
		button(code, 1);
		button(code, 0);
	} else if (strcmp(verb, "drag") == 0 && rest == 5) {
		// Through the middle, because a drag that teleports is one some
		// applications never register: they track motion, not the endpoints.
		long x1 = number(argv[2]), y1 = number(argv[3]);
		long x2 = number(argv[4]), y2 = number(argv[5]);
		uint32_t code = button_code(argv[6]);

		move_to(x1, y1);
		settle(20);
		button(code, 1);
		settle(20);
		move_to((x1 + x2) / 2, (y1 + y2) / 2);
		settle(20);
		move_to(x2, y2);
		settle(20);
		button(code, 0);
	} else if (strcmp(verb, "scroll") == 0 && rest == 3) {
		long notches = number(argv[4]);
		int down = notches > 0;
		long count = notches < 0 ? -notches : notches;

		move_to(number(argv[2]), number(argv[3]));
		settle(20);
		for (long sent = 0; sent < count; sent++) {
			wheel(down);
			settle(10);
		}
	} else {
		fputs(USAGE, stderr);
		return 2;
	}

	// Delivered before the device goes away with this process.
	wl_display_roundtrip(display);
	zwlr_virtual_pointer_v1_destroy(pointer);
	wl_display_roundtrip(display);
	wl_display_disconnect(display);
	return 0;
}
