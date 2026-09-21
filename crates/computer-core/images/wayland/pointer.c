// A virtual pointer lives only as long as its client, so one client stays and takes every gesture.

#define _GNU_SOURCE

#include "virtual-keyboard-unstable-v1-client-protocol.h"
#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/mman.h>
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
static struct zwp_virtual_keyboard_manager_v1 *keys_manager;
static struct zwp_virtual_keyboard_v1 *keys;

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
	} else if (strcmp(interface, zwp_virtual_keyboard_manager_v1_interface.name) == 0) {
		keys_manager = wl_registry_bind(registry, name,
		                                &zwp_virtual_keyboard_manager_v1_interface, 1);
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
    "                        type TEXT\n"
    "                        paced MS TEXT             (a pause of MS after each key)\n"
    "                        key [-M MOD | -m MOD | -k KEY | -P KEY | -p KEY] ...\n"
    "                        with MOD[,MOD] GESTURE    (the modifiers held through it)\n"
    "                        serve SOCKET              (the resident pointer of one screen)\n";

static const char *HOMELESS =
    "a button stays down only under the resident pointer, and this screen has none";

struct board_key {
	const char *name;
	uint32_t code;
	char plain;
	char shifted;
};

static const struct board_key BOARD[] = {
    {"AE01", 2, '1', '!'},  {"AE02", 3, '2', '@'},  {"AE03", 4, '3', '#'},  {"AE04", 5, '4', '$'},
    {"AE05", 6, '5', '%'},  {"AE06", 7, '6', '^'},  {"AE07", 8, '7', '&'},  {"AE08", 9, '8', '*'},
    {"AE09", 10, '9', '('}, {"AE10", 11, '0', ')'}, {"AE11", 12, '-', '_'}, {"AE12", 13, '=', '+'},
    {"AD01", 16, 'q', 'Q'}, {"AD02", 17, 'w', 'W'}, {"AD03", 18, 'e', 'E'}, {"AD04", 19, 'r', 'R'},
    {"AD05", 20, 't', 'T'}, {"AD06", 21, 'y', 'Y'}, {"AD07", 22, 'u', 'U'}, {"AD08", 23, 'i', 'I'},
    {"AD09", 24, 'o', 'O'}, {"AD10", 25, 'p', 'P'}, {"AD11", 26, '[', '{'}, {"AD12", 27, ']', '}'},
    {"AC01", 30, 'a', 'A'}, {"AC02", 31, 's', 'S'}, {"AC03", 32, 'd', 'D'}, {"AC04", 33, 'f', 'F'},
    {"AC05", 34, 'g', 'G'}, {"AC06", 35, 'h', 'H'}, {"AC07", 36, 'j', 'J'}, {"AC08", 37, 'k', 'K'},
    {"AC09", 38, 'l', 'L'}, {"AC10", 39, ';', ':'}, {"AC11", 40, '\'', '"'}, {"TLDE", 41, '`', '~'},
    {"BKSL", 43, '\\', '|'}, {"AB01", 44, 'z', 'Z'}, {"AB02", 45, 'x', 'X'}, {"AB03", 46, 'c', 'C'},
    {"AB04", 47, 'v', 'V'}, {"AB05", 48, 'b', 'B'}, {"AB06", 49, 'n', 'N'}, {"AB07", 50, 'm', 'M'},
    {"AB08", 51, ',', '<'}, {"AB09", 52, '.', '>'}, {"AB10", 53, '/', '?'},
};

#define BOARD_KEYS (sizeof BOARD / sizeof BOARD[0])
#define EXTRA_GROUPS 3
#define MOST_EXTRAS (BOARD_KEYS * EXTRA_GROUPS)
#define KEYMAP_SETTLE 120

#define KEY_SHIFT 42
#define MOD_SHIFT 1

struct named_key {
	const char *name;
	uint32_t code;
};

static const struct named_key NAMED[] = {
    {"Return", 28},   {"Enter", 28},   {"KP_Enter", 96}, {"Escape", 1},     {"space", 57},
    {"Tab", 15},      {"BackSpace", 14}, {"Delete", 111}, {"Insert", 110},  {"Up", 103},
    {"Down", 108},    {"Left", 105},   {"Right", 106},   {"Home", 102},     {"End", 107},
    {"Prior", 104},   {"Page_Up", 104}, {"Next", 109},   {"Page_Down", 109}, {"Menu", 127},
    {"Print", 99},    {"Pause", 119},  {"Caps_Lock", 58}, {"F1", 59},       {"F2", 60},
    {"F3", 61},       {"F4", 62},      {"F5", 63},       {"F6", 64},        {"F7", 65},
    {"F8", 66},       {"F9", 67},      {"F10", 68},      {"F11", 87},       {"F12", 88},
};

struct held_key {
	const char *name;
	uint32_t code;
	uint32_t mask;
};

static const struct held_key HELD[] = {
    {"shift", KEY_SHIFT, MOD_SHIFT}, {"ctrl", 29, 4}, {"control", 29, 4}, {"alt", 56, 8},
    {"super", 125, 64},              {"logo", 125, 64}, {"win", 125, 64}, {"meta", 125, 64},
};

struct spelled_key {
	const char *name;
	char is;
};

static const struct spelled_key SPELLED[] = {
    {"minus", '-'},      {"equal", '='},       {"plus", '+'},        {"underscore", '_'},
    {"bracketleft", '['}, {"bracketright", ']'}, {"braceleft", '{'}, {"braceright", '}'},
    {"semicolon", ';'},  {"colon", ':'},       {"apostrophe", '\''}, {"quotedbl", '"'},
    {"grave", '`'},      {"asciitilde", '~'},  {"backslash", '\\'},  {"bar", '|'},
    {"comma", ','},      {"period", '.'},      {"slash", '/'},       {"less", '<'},
    {"greater", '>'},    {"question", '?'},    {"exclam", '!'},      {"at", '@'},
    {"numbersign", '#'}, {"dollar", '$'},      {"percent", '%'},     {"asciicircum", '^'},
    {"ampersand", '&'},  {"asterisk", '*'},    {"parenleft", '('},   {"parenright", ')'},
};

static uint32_t extras[BOARD_KEYS * EXTRA_GROUPS];
static size_t extra_count;
static uint32_t mods;
static uint32_t keymap_at;

static const char *NO_KEYBOARD = "this compositor does not offer zwp_virtual_keyboard_v1";

static int load_keymap(void) {
	char *text = NULL;
	size_t size = 0;
	FILE *out = open_memstream(&text, &size);
	if (out == NULL) {
		return 1;
	}

	fputs("xkb_keymap {\n xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n"
	      " xkb_types { include \"complete\" };\n xkb_compat { include \"complete\" };\n"
	      " xkb_symbols { include \"pc+us+inet(evdev)\"\n",
	      out);
	for (size_t at = 0; at < BOARD_KEYS && at < extra_count; at++) {
		fprintf(out, "  key <%s> { symbols[Group1] = [ U%04X, U%04X ]", BOARD[at].name,
		        (unsigned)BOARD[at].plain, (unsigned)BOARD[at].shifted);
		for (size_t more = 0; more < EXTRA_GROUPS; more++) {
			size_t slot = at + more * BOARD_KEYS;
			if (slot < extra_count) {
				fprintf(out, ", symbols[Group%zu] = [ U%04X ]", more + 2, (unsigned)extras[slot]);
			}
		}
		fputs(" };\n", out);
	}
	fputs(" };\n};\n", out);
	if (fclose(out) != 0) {
		free(text);
		return 1;
	}

	int fd = memfd_create("computer-keymap", MFD_CLOEXEC);
	int failed = fd < 0 || write(fd, text, size + 1) != (ssize_t)size + 1;
	if (!failed) {
		zwp_virtual_keyboard_v1_keymap(keys, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, fd, size + 1);
		failed = wl_display_roundtrip(display) < 0;
		keymap_at = now_ms();
	}
	if (fd >= 0) {
		close(fd);
	}
	free(text);
	return failed;
}

static void keymap_settled(void) {
	uint32_t since = now_ms() - keymap_at;
	if (since < KEYMAP_SETTLE) {
		settle(KEYMAP_SETTLE - since);
	}
}

static void key_event(uint32_t code, uint32_t down) {
	zwp_virtual_keyboard_v1_key(keys, now_ms(), code, down);
}

static void state(uint32_t group) {
	zwp_virtual_keyboard_v1_modifiers(keys, mods, 0, 0, group);
}

static const struct held_key *held_named(const char *name) {
	for (size_t at = 0; at < sizeof HELD / sizeof HELD[0]; at++) {
		if (strcasecmp(name, HELD[at].name) == 0) {
			return &HELD[at];
		}
	}
	return NULL;
}

static void hold(const struct held_key *one, int down) {
	if (down) {
		key_event(one->code, 1);
		mods |= one->mask;
	} else {
		mods &= ~one->mask;
		key_event(one->code, 0);
	}
	state(0);
}

static int extra_slot(uint32_t point) {
	for (size_t at = 0; at < extra_count; at++) {
		if (extras[at] == point) {
			return (int)at;
		}
	}
	return -1;
}

static int on_board(uint32_t point, uint32_t *code, int *shifted) {
	for (size_t at = 0; at < BOARD_KEYS; at++) {
		if (point == (uint32_t)BOARD[at].plain || point == (uint32_t)BOARD[at].shifted) {
			*code = BOARD[at].code;
			*shifted = point == (uint32_t)BOARD[at].shifted;
			return 1;
		}
	}
	return 0;
}

static void tap(uint32_t code, int shifted, uint32_t group) {
	int lend = shifted && !(mods & MOD_SHIFT);
	if (lend) {
		hold(&HELD[0], 1);
	}
	if (group != 0) {
		state(group);
	}
	key_event(code, 1);
	key_event(code, 0);
	if (group != 0) {
		state(0);
	}
	if (lend) {
		hold(&HELD[0], 0);
	}
}

static size_t next_point(const char *text, uint32_t *point) {
	const unsigned char *at = (const unsigned char *)text;
	size_t width = at[0] < 0x80 ? 1 : (at[0] >> 5) == 0x6 ? 2 : (at[0] >> 4) == 0xE ? 3 : (at[0] >> 3) == 0x1E ? 4 : 0;

	if (width == 0) {
		*point = 0xFFFD;
		return 1;
	}
	*point = width == 1 ? at[0] : at[0] & (0xFF >> (width + 1));
	for (size_t more = 1; more < width; more++) {
		if ((at[more] & 0xC0) != 0x80) {
			*point = 0xFFFD;
			return more;
		}
		*point = (*point << 6) | (at[more] & 0x3F);
	}
	return width;
}

static int plain_key(uint32_t point) {
	uint32_t code;
	int shifted;
	return point == ' ' || point == '\n' || point == '\t' || point == '\r' ||
	       on_board(point, &code, &shifted);
}

static int learn(const char *text) {
	size_t before = extra_count;
	uint32_t point;

	for (const char *at = text; *at != '\0'; at += next_point(at, &point)) {
		next_point(at, &point);
		if (plain_key(point) || extra_slot(point) >= 0) {
			continue;
		}
		if (extra_count == MOST_EXTRAS) {
			break;
		}
		extras[extra_count++] = point;
	}

	if (extra_count == before) {
		return 0;
	}
	return load_keymap();
}

static const char *type_text(const char *text, long pause) {
	if (keys == NULL) {
		return NO_KEYBOARD;
	}
	if (learn(text) != 0) {
		return "the compositor would not take the keymap";
	}
	keymap_settled();

	uint32_t point;
	for (const char *at = text; *at != '\0'; at += next_point(at, &point)) {
		next_point(at, &point);

		uint32_t code = 0;
		int shifted = 0;
		if (point == '\r') {
			continue;
		} else if (point == ' ') {
			tap(57, 0, 0);
		} else if (point == '\n') {
			tap(28, 0, 0);
		} else if (point == '\t') {
			tap(15, 0, 0);
		} else if (on_board(point, &code, &shifted)) {
			tap(code, shifted, 0);
		} else {
			int slot = extra_slot(point);
			if (slot < 0) {
				extra_count = 0;
				if (learn(at) != 0) {
					return "the compositor would not take the keymap";
				}
				keymap_settled();
				slot = extra_slot(point);
			}
			tap(BOARD[(size_t)slot % BOARD_KEYS].code, 0, (uint32_t)slot / BOARD_KEYS + 1);
		}
		settle(pause > 0 ? pause : 2);
	}
	return NULL;
}

static const char *one_key(const char *name, int down, int up) {
	uint32_t code = 0, group = 0;
	int shifted = 0, found = 0;

	for (size_t at = 0; at < sizeof NAMED / sizeof NAMED[0] && !found; at++) {
		if (strcasecmp(name, NAMED[at].name) == 0) {
			code = NAMED[at].code;
			found = 1;
		}
	}

	uint32_t point = 0;
	for (size_t at = 0; at < sizeof SPELLED / sizeof SPELLED[0] && !found && point == 0; at++) {
		if (strcmp(name, SPELLED[at].name) == 0) {
			point = (uint32_t)SPELLED[at].is;
		}
	}
	if (!found && point == 0 && name[0] != '\0' && name[next_point(name, &point)] != '\0') {
		snprintf(fault, sizeof fault, "unknown key: %.100s", name);
		return fault;
	}

	if (!found && !on_board(point, &code, &shifted)) {
		char lone[5] = {0};
		memcpy(lone, name, strnlen(name, 4));
		if (learn(lone) != 0) {
			return "the compositor would not take the keymap";
		}
		int slot = extra_slot(point);
		if (slot < 0) {
			snprintf(fault, sizeof fault, "unknown key: %.100s", name);
			return fault;
		}
		code = BOARD[(size_t)slot % BOARD_KEYS].code;
		group = (uint32_t)slot / BOARD_KEYS + 1;
	}
	keymap_settled();

	int lend = shifted && down && !(mods & MOD_SHIFT);
	if (lend) {
		hold(&HELD[0], 1);
	}
	if (group != 0) {
		state(group);
	}
	if (down) {
		key_event(code, 1);
	}
	if (up) {
		key_event(code, 0);
	}
	if (group != 0) {
		state(0);
	}
	if (lend) {
		hold(&HELD[0], 0);
	}
	return NULL;
}

static const char *press_keys(int count, char **words) {
	if (keys == NULL) {
		return NO_KEYBOARD;
	}
	if (count == 0 || count % 2 != 0) {
		return USAGE;
	}
	keymap_settled();

	for (int at = 0; at + 1 < count; at += 2) {
		const char *how = words[at];
		const char *name = words[at + 1];
		const char *wrong = NULL;

		if (strcmp(how, "-M") == 0 || strcmp(how, "-m") == 0) {
			const struct held_key *one = held_named(name);
			if (one == NULL) {
				snprintf(fault, sizeof fault, "unknown modifier: %.100s", name);
				return fault;
			}
			hold(one, how[1] == 'M');
		} else if (strcmp(how, "-k") == 0) {
			wrong = one_key(name, 1, 1);
		} else if (strcmp(how, "-P") == 0) {
			wrong = one_key(name, 1, 0);
		} else if (strcmp(how, "-p") == 0) {
			wrong = one_key(name, 0, 1);
		} else {
			return USAGE;
		}

		if (wrong != NULL) {
			return wrong;
		}
		settle(2);
	}
	return NULL;
}

static char *unhex(const char *hex) {
	size_t size = strlen(hex) / 2;
	char *text = malloc(size + 1);
	if (text == NULL) {
		return NULL;
	}
	for (size_t at = 0; at < size; at++) {
		unsigned int byte = 0;
		if (sscanf(hex + at * 2, "%2x", &byte) != 1) {
			free(text);
			return NULL;
		}
		text[at] = (char)byte;
	}
	text[size] = '\0';
	return text;
}

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
	} else if (strcmp(verb, "type") == 0 && rest == 1) {
		return type_text(words[1], 0);
	} else if (strcmp(verb, "paced") == 0 && rest == 2) {
		long pause = number(words[1]);
		if (fault[0] != '\0') {
			return fault;
		}
		return type_text(words[2], pause);
	} else if ((strcmp(verb, "typehex") == 0 && rest == 1) ||
	           (strcmp(verb, "pacedhex") == 0 && rest == 2)) {
		long pause = rest == 2 ? number(words[1]) : 0;
		char *text = unhex(words[rest]);
		if (fault[0] != '\0' || text == NULL) {
			free(text);
			return fault[0] != '\0' ? fault : "the text did not arrive whole";
		}
		const char *wrong = type_text(text, pause);
		free(text);
		return wrong;
	} else if (strcmp(verb, "key") == 0) {
		return press_keys(rest, words + 1);
	} else if (strcmp(verb, "with") == 0 && rest >= 2 && strcmp(words[2], "with") != 0) {
		if (keys == NULL) {
			return NO_KEYBOARD;
		}

		const struct held_key *down[8];
		size_t held = 0;
		for (const char *at = words[1]; *at != '\0';) {
			size_t length = strcspn(at, ",");
			char name[16] = {0};
			if (length == 0 || length >= sizeof name || held == sizeof down / sizeof down[0]) {
				return USAGE;
			}
			memcpy(name, at, length);
			down[held] = held_named(name);
			if (down[held] == NULL) {
				snprintf(fault, sizeof fault, "unknown modifier: %.100s", name);
				return fault;
			}
			held++;
			at += length + (at[length] == ',');
		}

		keymap_settled();
		for (size_t at = 0; at < held; at++) {
			hold(down[at], 1);
		}
		settle(20);

		const char *wrong = gesture(count - 2, words + 2);
		static char kept[sizeof fault];
		if (wrong == fault) {
			memcpy(kept, fault, sizeof kept);
			wrong = kept;
		}

		settle(20);
		while (held > 0) {
			hold(down[--held], 0);
		}
		return wrong;
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
	if (keys_manager != NULL) {
		keys = zwp_virtual_keyboard_manager_v1_create_virtual_keyboard(keys_manager, seat);
	}

	// Before any event: events sent before the compositor makes the device are dropped.
	wl_display_roundtrip(display);

	if (keys != NULL && load_keymap() != 0) {
		fputs("the compositor would not take the keymap\n", stderr);
		return 1;
	}
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

	int typed = strcmp(words[0], "type") == 0 && count == 2;
	int paced = strcmp(words[0], "paced") == 0 && count == 3;
	if (typed || paced) {
		say(door, typed ? "typehex" : "pacedhex");
		if (paced) {
			say(door, " ");
			say(door, words[1]);
		}
		say(door, " ");
		for (const unsigned char *at = (const unsigned char *)words[count - 1]; *at != '\0'; at++) {
			char pair[3];
			snprintf(pair, sizeof pair, "%02x", *at);
			say(door, pair);
		}
		say(door, "\n");
	} else {
		for (int at_word = 0; at_word < count; at_word++) {
			say(door, words[at_word]);
			say(door, at_word + 1 < count ? " " : "\n");
		}
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
