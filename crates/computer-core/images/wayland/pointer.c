// A virtual pointer lives only as long as its client, so one client stays and takes every gesture.

#define _POSIX_C_SOURCE 200809L

#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
#include <wayland-client.h>

// linux/input-event-codes.h, which is what the protocol takes.
#define BTN_LEFT 0x110
#define BTN_RIGHT 0x111
#define BTN_MIDDLE 0x112

#define NOTCH 15

static struct wl_display *display;
static struct wl_seat *seat;
static struct wl_output *output;
static struct zwlr_virtual_pointer_manager_v1 *manager;
static struct zwlr_virtual_pointer_v1 *pointer;

static int resident;
static char fault[200];

// Read from the output: only the compositor knows the size the screen came up at.
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

// Forward is down or right, the sign the protocol uses for both axes.
static void wheel(uint32_t axis, int forward) {
	zwlr_virtual_pointer_v1_axis_source(pointer, WL_POINTER_AXIS_SOURCE_WHEEL);
	zwlr_virtual_pointer_v1_axis_discrete(pointer, now_ms(), axis,
	                                      wl_fixed_from_int(forward ? NOTCH : -NOTCH),
	                                      forward ? 1 : -1);
	zwlr_virtual_pointer_v1_frame(pointer);
}

static void wheel_by(uint32_t axis, long notches) {
	long count = notches < 0 ? -notches : notches;

	for (long sent = 0; sent < count; sent++) {
		wheel(axis, notches > 0);
		settle(10);
	}
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
	if (fault[0] == '\0') {
		snprintf(fault, sizeof fault, "unknown button: %.100s", name);
	}
	return 0;
}

static long number(const char *text) {
	char *end = NULL;
	long value = strtol(text, &end, 10);
	if (end == text || *end != '\0') {
		if (fault[0] == '\0') {
			snprintf(fault, sizeof fault, "not a number: %.100s", text);
		}
		return 0;
	}
	return value;
}

static const char *USAGE =
    "usage: computer-pointer move X Y\n"
    "                        click X Y BUTTON\n"
    "                        dblclick X Y BUTTON\n"
    "                        drag X1 Y1 X2 Y2 BUTTON\n"
    "                        path MS X Y [X Y ...]         (a pause of MS after each)\n"
    "                        sweep BUTTON MS X0 Y0 X Y [X Y ...]   (a drag through every point)\n"
    "                        scroll X Y DOWN [RIGHT]   (negative goes up and left)\n"
    "                        down BUTTON [X Y]         (stays down: needs the resident pointer)\n"
    "                        up BUTTON [X Y]\n"
    "                        serve SOCKET              (the resident pointer of one screen)\n";

static const char *HOMELESS =
    "a button stays down only under the resident pointer, and this screen has none";

static long *numbers(int count, char **words) {
	long *read = calloc((size_t)count + 1, sizeof *read);
	if (read == NULL) {
		snprintf(fault, sizeof fault, "out of memory");
		return NULL;
	}
	for (int at = 0; at < count; at++) {
		read[at] = number(words[at]);
	}
	return read;
}

static const char *gesture(int count, char **words) {
	fault[0] = '\0';

	if (count < 1) {
		return USAGE;
	}

	const char *verb = words[0];
	int rest = count - 1;

	if (strcmp(verb, "move") == 0 && rest == 2) {
		long x = number(words[1]), y = number(words[2]);
		if (fault[0] != '\0') {
			return fault;
		}
		move_to(x, y);
	} else if ((strcmp(verb, "click") == 0 || strcmp(verb, "dblclick") == 0) && rest == 3) {
		long x = number(words[1]), y = number(words[2]);
		uint32_t code = button_code(words[3]);
		if (fault[0] != '\0') {
			return fault;
		}
		move_to(x, y);
		settle(20);
		button(code, 1);
		button(code, 0);
		// One run: two runs are far enough apart to read as two single clicks.
		if (strcmp(verb, "dblclick") == 0) {
			settle(40);
			button(code, 1);
			button(code, 0);
		}
	} else if (strcmp(verb, "drag") == 0 && rest == 5) {
		// Through the middle: some applications track motion, not the endpoints.
		long x1 = number(words[1]), y1 = number(words[2]);
		long x2 = number(words[3]), y2 = number(words[4]);
		uint32_t code = button_code(words[5]);
		if (fault[0] != '\0') {
			return fault;
		}

		move_to(x1, y1);
		settle(20);
		button(code, 1);
		settle(20);
		move_to((x1 + x2) / 2, (y1 + y2) / 2);
		settle(20);
		move_to(x2, y2);
		settle(20);
		button(code, 0);
	} else if (strcmp(verb, "path") == 0 && rest >= 3 && rest % 2 == 1) {
		long *read = numbers(rest, words + 1);
		if (fault[0] != '\0') {
			free(read);
			return fault;
		}

		for (int at = 1; at + 1 < rest; at += 2) {
			move_to(read[at], read[at + 1]);
			settle(read[0]);
		}
		free(read);
	} else if (strcmp(verb, "sweep") == 0 && rest >= 6 && rest % 2 == 0) {
		uint32_t code = button_code(words[1]);
		long *read = numbers(rest - 1, words + 2);
		if (fault[0] != '\0') {
			free(read);
			return fault;
		}

		move_to(read[1], read[2]);
		settle(20);
		button(code, 1);
		settle(20);
		for (int at = 3; at + 1 < rest - 1; at += 2) {
			move_to(read[at], read[at + 1]);
			settle(read[0]);
		}
		settle(20);
		button(code, 0);
		free(read);
	} else if (strcmp(verb, "scroll") == 0 && (rest == 3 || rest == 4)) {
		long x = number(words[1]), y = number(words[2]);
		long down = number(words[3]);
		long right = rest == 4 ? number(words[4]) : 0;
		if (fault[0] != '\0') {
			return fault;
		}

		move_to(x, y);
		settle(20);
		wheel_by(WL_POINTER_AXIS_VERTICAL_SCROLL, down);
		wheel_by(WL_POINTER_AXIS_HORIZONTAL_SCROLL, right);
	} else if ((strcmp(verb, "down") == 0 || strcmp(verb, "up") == 0) && (rest == 1 || rest == 3)) {
		if (!resident) {
			return HOMELESS;
		}

		uint32_t code = button_code(words[1]);
		long x = rest == 3 ? number(words[2]) : 0;
		long y = rest == 3 ? number(words[3]) : 0;
		if (fault[0] != '\0') {
			return fault;
		}

		if (rest == 3) {
			move_to(x, y);
			settle(20);
		}
		button(code, strcmp(verb, "down") == 0 ? 1 : 0);
	} else {
		return USAGE;
	}

	return NULL;
}

static int meet_compositor(void) {
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

	// Before any event: events sent before the compositor makes the device are dropped.
	wl_display_roundtrip(display);
	return 0;
}

static int door_address(struct sockaddr_un *at, const char *path) {
	memset(at, 0, sizeof *at);
	at->sun_family = AF_UNIX;
	if (strlen(path) >= sizeof at->sun_path) {
		fprintf(stderr, "%s is too long for a socket\n", path);
		return 1;
	}
	strcpy(at->sun_path, path);
	return 0;
}

static int resident_path(char *path, size_t size) {
	const char *named = getenv("COMPUTER_POINTER_SOCKET");
	if (named != NULL && named[0] != '\0') {
		return snprintf(path, size, "%s", named) < (int)size ? 0 : 1;
	}

	const char *runtime = getenv("XDG_RUNTIME_DIR");
	if (runtime == NULL || runtime[0] == '\0') {
		return 1;
	}
	return snprintf(path, size, "%s/computer-pointer", runtime) < (int)size ? 0 : 1;
}

static int knock(const char *path) {
	struct sockaddr_un at;
	if (door_address(&at, path) != 0) {
		return -1;
	}

	int door = socket(AF_UNIX, SOCK_STREAM, 0);
	if (door >= 0 && connect(door, (struct sockaddr *)&at, sizeof at) != 0) {
		close(door);
		return -1;
	}
	return door;
}

static void revive(const char *path) {
	if (fork() != 0) {
		return;
	}

	setsid();
	int nowhere = open("/dev/null", O_RDWR);
	if (nowhere >= 0) {
		dup2(nowhere, 0);
		dup2(nowhere, 1);
		dup2(nowhere, 2);
	}
	execlp("computer-pointer", "computer-pointer", "serve", path, (char *)NULL);
	_exit(127);
}

static void say(int caller, const char *text) {
	size_t left = strlen(text);
	while (left > 0) {
		ssize_t sent = write(caller, text, left);
		if (sent <= 0) {
			return;
		}
		text += sent;
		left -= (size_t)sent;
	}
}

#define LONGEST_LINE (1 << 20)

static char *hear(int caller) {
	size_t held = 0, room = 4096;
	char *line = malloc(room);

	while (line != NULL) {
		ssize_t got = read(caller, line + held, room - held - 1);
		if (got < 0 && errno == EINTR) {
			continue;
		}
		if (got <= 0) {
			break;
		}
		held += (size_t)got;
		if (memchr(line, '\n', held) != NULL) {
			break;
		}
		if (held + 1 >= room) {
			if (room >= LONGEST_LINE) {
				free(line);
				return NULL;
			}
			room *= 2;
			char *more = realloc(line, room);
			if (more == NULL) {
				free(line);
				return NULL;
			}
			line = more;
		}
	}

	if (line != NULL) {
		line[held] = '\0';
		line[strcspn(line, "\n")] = '\0';
	}
	return line;
}

static void answer(int caller) {
	char *line = hear(caller);
	if (line == NULL) {
		say(caller, "no: the gesture is longer than this pointer reads\n");
		return;
	}

	size_t most = strlen(line) / 2 + 2;
	char **words = calloc(most, sizeof *words);
	int count = 0;
	if (words != NULL) {
		for (char *word = strtok(line, " "); word != NULL; word = strtok(NULL, " ")) {
			words[count++] = word;
		}
	}

	const char *wrong = words == NULL ? "out of memory" : gesture(count, words);
	if (wl_display_roundtrip(display) < 0) {
		wrong = "the compositor went away";
	}

	if (wrong == NULL) {
		say(caller, "ok\n");
	} else {
		say(caller, "no: ");
		say(caller, wrong);
		say(caller, "\n");
	}

	free(words);
	free(line);
}

static int serve(const char *path) {
	struct sockaddr_un at;
	if (door_address(&at, path) != 0) {
		return 2;
	}

	signal(SIGPIPE, SIG_IGN);

	int other = knock(path);
	if (other >= 0) {
		close(other);
		return 0;
	}

	int door = socket(AF_UNIX, SOCK_STREAM, 0);
	unlink(path);
	if (door < 0 || bind(door, (struct sockaddr *)&at, sizeof at) != 0 || listen(door, 16) != 0) {
		perror(path);
		return 1;
	}

	struct pollfd watched[2] = {
	    {.fd = door, .events = POLLIN},
	    {.fd = wl_display_get_fd(display), .events = POLLIN},
	};

	for (;;) {
		while (wl_display_prepare_read(display) != 0) {
			wl_display_dispatch_pending(display);
		}
		wl_display_flush(display);

		if (poll(watched, 2, -1) < 0) {
			wl_display_cancel_read(display);
			if (errno == EINTR) {
				continue;
			}
			return 1;
		}

		if (watched[1].revents & POLLIN) {
			if (wl_display_read_events(display) < 0) {
				return 1;
			}
			wl_display_dispatch_pending(display);
		} else {
			wl_display_cancel_read(display);
		}
		if (watched[1].revents & (POLLHUP | POLLERR)) {
			return 1;
		}

		if (watched[0].revents & POLLIN) {
			int caller = accept(door, NULL, NULL);
			if (caller >= 0) {
				answer(caller);
				close(caller);
			}
		}
	}
}

static int forward(int count, char **words, int *status) {
	char path[sizeof(((struct sockaddr_un *)0)->sun_path)];
	if (resident_path(path, sizeof path) != 0) {
		return 0;
	}

	int door = knock(path);
	if (door < 0) {
		revive(path);
		for (int tries = 0; tries < 40 && door < 0; tries++) {
			struct timespec wait = {.tv_nsec = 50 * 1000000};
			nanosleep(&wait, NULL);
			door = knock(path);
		}
	}
	if (door < 0) {
		return 0;
	}

	signal(SIGPIPE, SIG_IGN);
	for (int at_word = 0; at_word < count; at_word++) {
		say(door, words[at_word]);
		say(door, at_word + 1 < count ? " " : "\n");
	}

	char reply[1200];
	size_t held = 0;
	for (;;) {
		ssize_t got = read(door, reply + held, sizeof reply - held - 1);
		if (got < 0 && errno == EINTR) {
			continue;
		}
		if (got <= 0) {
			break;
		}
		held += (size_t)got;
		if (held + 1 >= sizeof reply) {
			break;
		}
	}
	reply[held] = '\0';
	close(door);

	if (strncmp(reply, "ok", 2) == 0) {
		*status = 0;
	} else if (strncmp(reply, "no: ", 4) == 0) {
		fputs(reply + 4, stderr);
		*status = strncmp(reply + 4, "usage:", 6) == 0 ? 2 : 1;
	} else {
		fputs("the resident pointer went away mid-gesture\n", stderr);
		*status = 1;
	}
	return 1;
}

int main(int argc, char **argv) {
	if (argc < 2) {
		fputs(USAGE, stderr);
		return 2;
	}

	if (strcmp(argv[1], "serve") == 0) {
		if (argc != 3) {
			fputs(USAGE, stderr);
			return 2;
		}
		resident = 1;
		if (meet_compositor() != 0) {
			return 1;
		}
		return serve(argv[2]);
	}

	int status = 0;
	if (forward(argc - 1, argv + 1, &status)) {
		return status;
	}

	if (meet_compositor() != 0) {
		return 1;
	}

	const char *wrong = gesture(argc - 1, argv + 1);

	// Delivered before the device goes away with this process.
	wl_display_roundtrip(display);
	zwlr_virtual_pointer_v1_destroy(pointer);
	wl_display_roundtrip(display);
	wl_display_disconnect(display);

	if (wrong != NULL) {
		fputs(wrong, stderr);
		if (wrong != USAGE) {
			fputc('\n', stderr);
		}
		return wrong == USAGE ? 2 : 1;
	}
	return 0;
}
