# CLI reference

`holm` creates and drives boxes. Run `holm --help` for the same list in the terminal.

```text
holm [--server URL] [--local] <command> [args…]
```

## Global options

| Option | Effect |
| --- | --- |
| `--server URL` | Use this server. It overrides `HOLM_SERVER_URL`. |
| `--local` | Drive the local runtime from the CLI process, with no server. See [`--local`](#--local). |

## Server selection

Most commands go through the REST server. `holm` selects the server in this order:

1. `--server URL`
2. `HOLM_SERVER_URL`
3. A `holmd` on `http://127.0.0.1:8080`
4. A server that starts for the command and stops when it exits

The CLI checks that the service on port 8080 is `holmd`. It does not use an unrelated process on that port.

`HOLM_SERVER_TOKEN` is sent as a bearer token.

Run `holmd` when you need state across commands: traces for `trace` and `fork`, box expiry, and access from other hosts.

## Output

Primary values, such as a box ID, a tab ID, or a URL, go to standard output. Status text goes to standard error. You can use a command inside another:

```bash
holm screenshot "$(holm new)" out.png
```

## Boxes

| Command | Effect |
| --- | --- |
| `new [options]` | Open a box. Prints the box ID, and the viewer URL on standard error. |
| `ls` | List running boxes. |
| `box <box>` | Show all that the server knows about a box. |
| `pause <box>` | Freeze the box. It keeps its memory and ports, and uses no CPU until you resume it. |
| `stop <box>` | Stop all processes and keep the files. This uses less than `pause`, which keeps the memory. |
| `resume <box>` | Make a paused or stopped box usable. A stopped box starts a new desktop on a new viewer URL. |
| `rm <box>` | Remove the box. |
| `sweep` | Remove each box whose deadline has passed. Always local, on the default Docker runtime. |

### `new` options

| Option | Effect |
| --- | --- |
| `--size WxH` | Screen size. Default `1280x800`. |
| `--screens N` | Number of screens. |
| `--url URL` | Open this URL when the desktop starts. |
| `--app NAME` | Install an application from the catalog, such as `gimp` or `vscode`. Repeat it, or use `a,b`. |
| `--package PKG` | Install a system package. Repeat it, or use `a,b`. |
| `--audio` | Install PulseAudio. |
| `--base` | The default desktop: Noto CJK and color emoji fonts, ffmpeg for `record`, a tint2 dock, AT-SPI for `widget`, and XWayland in a `--wayland` box. |
| `--minimal` | The bare desktop, with none of the `--base` features. |
| `--wayland` | Run sway instead of X11. |
| `--no-network` | Block outbound network access. |
| `--memory SIZE` | Memory limit, such as `2g`. |
| `--cpus N` | CPU limit. |
| `--runtime NAME` | Put the box on this runtime. See `runtime ls`. |
| `--ttl MINUTES` | Remove the box this long after it opens. |
| `--idle MINUTES` | Remove the box this long after its last use. |
| `--persistent` | Keep the files of the box when it stops, so that a resume brings it back. Only on a runtime that can. |
| `--profile NAME` | Keep browser logins, cookies, and history in a volume with this name. The next box with the same name starts with them. Only one box at a time can use a profile. |
| `--spec FILE` | Read a box specification, in the shape of the `POST /v1/boxes` body. Use `-` for standard input. A flag overrides the file. |

You cannot change `--app`, `--package`, a feature, or `--wayland` after a box opens. The first box with a new application, package, or feature builds a new image, which takes a few minutes.

## Runtimes and images

| Command | Effect |
| --- | --- |
| `runtime ls` | List the runtimes that can hold a box, and what each can do. |
| `runtime add NAME --provider VENDOR [--field name=value]… [--api-key \| --api-key-env VAR]` | Add a cloud vendor to the server. |
| `runtime set NAME [--field name=value]… [--api-key \| --api-key-env VAR]` | Change the fields or the key of a vendor. |
| `runtime rm NAME` | Remove a vendor. The server refuses while a box uses it. |
| `image build NAME [--app NAME]… [--package PKG]… [--spec FILE]` | Build the image for a runtime before a box needs it. |
| `image ls [NAME]` | List the images the server built, and what each came from. |
| `image rm NAME IMAGE` | Remove an image. The server refuses while a box uses it. |

`--api-key` reads the key from standard input. `--api-key-env VAR` reads it from an environment variable. The key never goes on the command line, where `ps` and the shell history can show it.

`image build` works differently for each runtime type. A container engine builds the image on the host. A hypervisor takes a copy. A vendor that builds its own images gets the build instructions.

## Screen

| Command | Effect |
| --- | --- |
| `screenshot <box> [file.png]` | Capture the screen. |
| `wait <box> [--settle MS] [--within MS]` | Wait until the screen is unchanged for `--settle`, or stop after `--within`. |
| `record <box> start [--fps N]` | Start a screen recording. Needs video, which a box has unless it was opened with `--minimal`. |
| `record <box> stop [file.mp4]` | Stop the recording and copy the file out. |
| `record <box> status` | Show the recording state. |

### `screenshot` options

| Option | Effect |
| --- | --- |
| `--window ID` | Capture one window. |
| `--at X,Y --size WxH` | Capture a rectangle. |
| `--scale PERCENT` | Return a smaller image. |
| `--pointer` | Draw the pointer. A capture does not show it otherwise. |
| `--tab ID` | Bring this browser tab to the front first. |

## Mouse

Coordinates are device pixels. `(0, 0)` is the top-left corner. `button` is `left`, `right`, or `middle`.

| Command | Effect |
| --- | --- |
| `mouse <box> move <x> <y> [--smooth \| --human] [--seed N]` | Move the pointer. It jumps unless you give `--smooth` (a straight line) or `--human` (a curve). The same `--seed` gives the same curve. |
| `mouse <box> click <x> <y> [button] [--double] [--held shift,ctrl]` | Click. `--held` keeps modifier keys down during the click. |
| `mouse <box> drag <x> <y> <x> <y> [button] [--held …]` | Press at the first point and release at the second. |
| `mouse <box> path <x> <y> <x> <y> … [button] [--held …]` | One press, through each point, then one release. Accepts `--smooth` and `--human`. |
| `mouse <box> down [<x> <y>] [button] [--hold SECONDS]` | Press a button. |
| `mouse <box> up [<x> <y>] [button]` | Release a button. |
| `mouse <box> scroll [<x> <y>] up\|down\|left\|right [notches]` | Turn the wheel. Default 3 notches, at the center of the screen. |
| `mouse <box> scroll <x> <y> <dy> [dx]` | Turn the wheel on both axes. Positive is down and right. |
| `mouse <box> at` | Show the pointer position. |

The server releases a button that `down` pressed after `--hold` seconds (default 10, maximum 60), or when a person takes control. `down` and `up` need a server.

## Keyboard

| Command | Effect |
| --- | --- |
| `keyboard <box> type <text> [--delay MS]` | Type into the focused window. `--delay` adds time between keys, for inputs that lose characters. |
| `keyboard <box> press <keys…> [--held shift,alt]` | Press one key or a combination, such as `enter`, `ctrl+l`, or `cmd+shift+p`. `--held` keeps modifiers down across all the keys. |
| `keyboard <box> down <key> [--hold SECONDS]` | Press a key. |
| `keyboard <box> up <key>` | Release a key. |

`press tab tab tab --held alt` goes to the third window. Three separate `press alt+tab` commands go only to the second, because each releases `alt`.

The server releases a key that `down` pressed after `--hold` seconds (default 10, maximum 60), or when a person takes control.

## Clipboard

| Command | Effect |
| --- | --- |
| `clip <box>` | Read the clipboard. |
| `clip <box> <text>` | Set the clipboard. |
| `--primary` | Use the primary selection, not the clipboard. |

## Windows and applications

| Command | Effect |
| --- | --- |
| `apps` | List the application names that a box can have. |
| `app <box> <name> [args…]` | Open an application and wait until it draws. |
| `app <box> install <name>…` | Install catalog applications into the running box. |
| `window <box> list` | List the windows on the screen. |
| `window <box> active` | Show the window that gets keyboard input. |
| `window <box> wait <class> [--within SECONDS]` | Wait for a window to appear and stop moving. |
| `window <box> <id> focus \| close` | Bring a window to the front, or close it. |
| `window <box> <id> move <x> <y>` | Move a window. |
| `window <box> <id> size <w> <h>` | Resize a window. |
| `window <box> <id> max \| min \| restore` | Change the window state. |

## Native widgets

These commands drive a native window by the names of its widgets. They need accessibility, which a box has unless it was opened with `--minimal`. A query matches the label next to a field and the name of the widget.

| Command | Effect |
| --- | --- |
| `widget <box> find <query> [--role R] [--exact] [--app NAME] [--limit N]` | Find widgets. |
| `widget <box> tree [--app NAME] [--depth N]` | Show the widget tree. |
| `widget <box> press <query> [--action NAME]` | Run the widget's action. This sends no pointer event, so it works on a covered widget. |
| `widget <box> fill <query> <value>` | Set the value of a field. |
| `widget <box> focus <query>` | Give a widget the keyboard focus. |

## Browser

These commands drive a web page by its content, not its pixels. A query is text, a name, an ID, a CSS selector, or a reference from `snapshot` such as `@e12`. Each action reports the controls that it made appear or go away.

Most commands take `--tab ID` to act on a tab that is not at the front. A tab that `open --label NAME` named also accepts that name.

### Pages and tabs

| Command | Effect |
| --- | --- |
| `open <box> <url> [--target blank\|current] [--label NAME]` | Open a URL. Prints the tab ID. A new tab unless you give `--target current`. |
| `browser <box> tabs` | List tabs. |
| `browser <box> switch <tab>` | Bring a tab to the front. |
| `browser <box> close <tab>` | Close a tab. |
| `browser <box> back \| forward \| reload` | Move in the history, or reload. |

### Read

| Command | Effect |
| --- | --- |
| `browser <box> read [--format text\|raw] [--limit N]` | Read the page as text. |
| `browser <box> snapshot [--scope QUERY] [--limit N] [--urls] [--delta] [--quiet MS]` | List the controls on the page in order, with a reference for each. `--urls` adds link addresses. `--delta` shows only what changed since the last snapshot. `--quiet` waits for the page to stop changing. |
| `browser <box> find [<query>] [--role R] [--exact] [--scroll] [--limit N]` | Find elements. The result names each element exactly. |
| `browser <box> eval <expression> [--timeout MS] [--limit N]` | Run JavaScript in the page. |
| `browser <box> console [--errors] [--clear] [--limit N]` | Show what the page logged since it loaded. `--clear` empties the log after it is read. |
| `browser <box> errors` | The same as `console --errors`. |

Add `--content-boundaries` to `read`, `snapshot`, `find`, or `eval` to put page text between two markers with a nonce that the page cannot know. Set `HOLM_CONTENT_BOUNDARIES=1` to do this for every call.

### Act

| Command | Effect |
| --- | --- |
| `browser <box> click <query> [--double] [--button right] [--human] [--new-tab]` | Click an element. `--new-tab` opens a link in a new tab and brings it to the front. |
| `browser <box> hover <query> [--human]` | Move the pointer over an element. |
| `browser <box> drag <query> <query> [--button right] [--human]` | Drag one element to another. |
| `browser <box> fill <query> <value>` | Type into a field, or set a slider, color, date, or time. Dropdowns, checkboxes, file inputs, and buttons are refused. |
| `browser <box> focus <query>` | Give an element the keyboard focus with no click. |
| `browser <box> check <query> \| uncheck <query>` | Set or clear a checkbox. No click occurs if it is already in that state. |
| `browser <box> select <query> <option…>` | Select dropdown options by text or value. The options you give become the full selection. |
| `browser <box> deselect <query> [<option…>]` | Clear the given options, or all options. |
| `browser <box> options <query>` | List the options of a dropdown. |
| `browser <box> upload <query> <file…> [--in-box]` | Give files to a file input. Paths are on the host unless you give `--in-box`. |
| `browser <box> dialog accept [text] \| dismiss` | Answer an alert, confirm, or prompt. No page command works while a dialog is open. |
| `browser <box> highlight <query> [--for SECONDS]` | Draw a box around an element. Default 3 seconds, maximum 60. |

### Wait

```text
browser <box> wait [<query>] [--gone] [--or TEXT,TEXT] [--within MS] [--quiet MS] [--enabled] [--load] [--fn JS]
```

| Option | Waits for |
| --- | --- |
| `<query>` | The element to appear. |
| `--gone` | The element to go away. |
| `--or TEXT,TEXT` | Any of these texts. |
| `--enabled` | The element to accept input. |
| `--load` | The document to load. |
| `--fn JS` | A JavaScript expression to be truthy. |
| `--quiet MS` | The page to stop changing for this time. |
| `--within MS` | The time limit. |

### Capture

| Command | Effect |
| --- | --- |
| `browser <box> screenshot [file] [--full] [--format png\|jpeg] [--quality N] [--annotate]` | Capture the page with no window frame or address bar. `--full` captures the full scrollable page, as JPEG by default. `--annotate` draws each control's reference from the last snapshot. |
| `browser <box> pdf [out.pdf] [--landscape] [--no-background]` | Print the page to a PDF, with text as text. |

### State and cookies

| Command | Effect |
| --- | --- |
| `browser <box> state save <file> [--origin URL]… [--session-storage] [--indexed-db] [--no-local-storage]` | Save cookies and storage to a file. With no `--origin`, the origins of the open tabs. |
| `browser <box> state save --name NAME [--origin URL]…` | Save to the server under a name, until the server restarts. |
| `browser <box> state load <file> \| --name NAME` | Load saved state into a box. |
| `browser state list` | List saved names. |
| `browser state rm <name>` | Remove a saved name. |
| `browser <box> cookies [--url URL]` | List cookies, or only those sent to a URL. |
| `browser <box> cookies set NAME=VALUE… --url URL [--domain D] [--path P] [--secure] [--http-only] [--expires SECONDS]` | Set cookies. |
| `browser <box> cookies set --curl '<curl command>'` | Set cookies from a browser's "Copy as cURL". |
| `browser <box> cookies clear --url URL \| --all` | Clear cookies. |

A state file contains live logins. The CLI writes it with mode `0600`.

### Other libraries

```text
cdp <box> [--ws] [--ttl MINUTES] [--direct]
```

Prints a Chrome DevTools address for another library, such as `agent-browser --cdp`, Playwright `connectOverCDP`, or browser-use `cdp_url`.

| Option | Effect |
| --- | --- |
| (none) | An address through the server, with a short-lived token valid for one hour. |
| `--ttl MINUTES` | Change the token lifetime. Maximum 24 hours. |
| `--ws` | The browser's WebSocket address, for a library that needs one. |
| `--direct` | The box's own port. Only this machine can reach it, and it has no protection. |

## Files and commands

| Command | Effect |
| --- | --- |
| `file <box> ls [dir]` | List a directory. Default `/`. |
| `file <box> get <path> [out]` | Copy a file out. To standard output if you give no `out`. |
| `file <box> put <local> [path]` | Copy a file in. Default `/tmp/<name>`. |
| `file <box> grep <pattern> <dir> [--include GLOB] [--ignore-case] [--limit N]` | Search in files. The directory is necessary. |
| `file <box> glob <pattern> [dir] [--limit N]` | Find files by name. |
| `exec <box> -- <command…>` | Run a command in the box. |

`grep` and `glob` stop at 200 results and tell you when they stopped.

## Human control

| Command | Effect |
| --- | --- |
| `takeover <box>` | Open the input viewer and print its URL. A person can then use the mouse and keyboard. |
| `release <box>` | Close the viewer and give control back. |

## Batches, traces, and forks

| Command | Effect |
| --- | --- |
| `batch <box> [file.json] [--keep-going] [--settle MS]` | Run many steps in one call, from a file or standard input. |
| `trace <box> [--after SEQ]` | Show what was done to a box, and by whom. |
| `fork <box> [--up-to SEQ]` | Make a new box by doing again what was done to this one. |

A batch holds the screen for the full run, which separate commands cannot do. Each command above is a step. `--keep-going` continues after a step is refused. `mouse_down` and `mouse_up` are steps only in a batch. A button that is down when the batch ends is released, and the result says so. `pause_ms` on a move step waits after the move.

A fork does the actions again. It does not copy the box, so the two boxes are similar but not always identical.

`trace` and `fork` need a server that keeps traces, such as `holmd`.

## MCP

| Command | Effect |
| --- | --- |
| `mcp --stdio` | Serve MCP over stdio. Logs go to standard error. Standard output carries only JSON-RPC. |

## `--local`

`--local` drives the local runtime from the CLI process, with no server. It supports only these commands:

`new`, `ls`, `screenshot`, `open`, `mouse`, `keyboard`, `wait`, `clip`, `takeover`, `release`, `exec`, `rm`, `sweep`

The other commands need a server and refuse `--local`. `mouse down` and `mouse up` also need a server, because the server releases a button that stays down too long.

## Environment

| Variable | Effect |
| --- | --- |
| `HOLM_SERVER_URL` | The server to use. |
| `HOLM_SERVER_TOKEN` | The bearer token for the server. |
| `HOLM_CONTENT_BOUNDARIES` | `1` puts page text between nonce markers in `read`, `snapshot`, `find`, and `eval`. |
