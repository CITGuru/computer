# MCP tools reference

`holm mcp --stdio` and `holmd` at `/mcp` serve the same 63 tools. To connect a host, see the [MCP quick start](../getting-started/quickstart-mcp.md).

A parameter with `*` is necessary. All other parameters are optional.

## Common parameters

Almost all tools take these parameters. The tables below do not show them again.

| Parameter | Type | Effect |
| --- | --- | --- |
| `box_id`* | string | The box, from `launch_box` or `list_boxes`. All tools except `launch_box`, `list_boxes`, `list_apps`, and `list_runtimes` need it. |
| `screenshot` | `auto` \| `always` \| `never` | When the result includes the screen. `auto` (default) sends it when the screen changed or an action was refused. `never` captures no frame. |
| `have_frame` | string | The `frame` hash from the last result. If the screen did not change, the result says `unchanged` and sends no image. |

Tools that change the screen accept `screenshot` and `have_frame`, and return the frame they produced.

Page tools also take:

| Parameter | Type | Effect |
| --- | --- | --- |
| `tab` | string | The page, by ID or label from `tabs` or `open_url`. Default: the page at the front. Give it when you can: it is safer and faster. |
| `query` | string | An element, by its text, name, ID, placeholder, CSS selector, or a reference from `snapshot` such as `@e12`. |

Pointer tools also take:

| Parameter | Type | Effect |
| --- | --- | --- |
| `motion` | `instant` \| `smooth` \| `human` | How the pointer moves. `instant` (default) jumps. `smooth` moves on a line. `human` moves on a curve over 100 to 700 ms. |
| `seed` | integer | For `human`: the same seed gives the same curve. |
| `button` | `left` \| `right` \| `middle` | The mouse button. Default `left`. |

## Errors

A tool that fails returns `isError` with the reason, so the model can read it and act. Only a malformed call returns a JSON-RPC error.

## Boxes

### `launch_box`

Start a Linux desktop with a browser. Returns the box ID and a viewer URL. Remove the box with `remove_box` when you are done.

| Parameter | Type | Effect |
| --- | --- | --- |
| `width`, `height` | integer | Screen size. Default: the image's size. |
| `screens` | integer | Number of screens. Default 1. MCP tools act on screen 0 only; use REST for other screens. |
| `runtime` | string | A runtime name from `list_runtimes`. Default: the server's default. |
| `apps` | string[] | Applications from `list_apps`, such as `gimp` or `vscode`. |
| `packages` | string[] | Apt packages, such as `jq` or `ripgrep`. |
| `accessibility` | boolean | Let `widget` read native windows. On unless `false`. |
| `video` | boolean | ffmpeg, for `record`. On unless `false`. |
| `audio` | boolean | A sound server. Off unless `true`. |
| `wide_fonts` | boolean | Chinese, Japanese, Korean, and emoji fonts. On unless `false`. |
| `minimal` | boolean | The bare desktop, without the fonts, ffmpeg, dock, and accessibility. A `true` feature flag adds that feature back. |
| `wayland` | boolean | Run sway instead of X11. |
| `network` | boolean | `false` removes all network access. Default `true`. |
| `memory` | string | Memory limit, such as `4g`. |
| `cpus` | string | CPU limit, such as `2`. |
| `profile` | string | A browser profile name. Logins, cookies, and history stay after the box is removed, and the next box with this name gets them. Only one box at a time can use a profile. |
| `ttl_minutes` | integer | Remove the box this long after it opens. |
| `idle_minutes` | integer | Remove the box after this long with no use. |

You cannot add `apps`, `accessibility`, or `video` to a running box. The first box with a new set of applications or packages builds an image, which takes minutes.

### Other box tools

| Tool | Effect |
| --- | --- |
| `list_boxes` | List running boxes. |
| `inspect_box` | Show the state, size, creation time, expiry, and viewer URL of a box. |
| `list_runtimes` | List the runtimes that can hold a box, and what each can do, such as pause. |
| `list_apps` | List the application names that `launch_box` accepts. |
| `pause_box` | Freeze a box. It keeps memory, ports, and open windows, and uses no CPU. Other tools wait, and do not fail, on a paused box, so resume it first. |
| `stop_box` | Stop a box and keep its files. The memory is released. A resumed box starts a new desktop with a new viewer URL. |
| `resume_box` | Make a paused or stopped box usable. The result says which desktop you got. |
| `remove_box` | Remove a box. Its files are deleted. |
| `fork_box` | Make a new box by doing again what was done to this box. The two boxes are similar, but not always identical. |

## Screen

### `screenshot`

Capture the desktop, one window, or a rectangle. Use the full-size image to get click coordinates: `(0, 0)` is the top-left corner, in device pixels. Do not aim from a cropped or scaled image.

| Parameter | Type | Effect |
| --- | --- | --- |
| `window` | string | A window ID from `window` with `op: list`. |
| `x`, `y`, `width`, `height` | integer | A rectangle of the screen. |
| `scale` | integer | Percent of full size, 1 to 400. |
| `pointer` | boolean | Draw the pointer. |
| `tab` | string | Bring this page to the front first. |

### `wait_until_still`

Wait until the screen stops changing. Use it after a menu opens, a dialog appears, or a page draws. A screen with an animation reaches the time limit.

| Parameter | Type | Effect |
| --- | --- | --- |
| `settle_ms` | integer | How long the screen must not change. Default 400. |
| `within_ms` | integer | The time limit. Default 10000. |

### `record`

Record the screen to a video file in the box. Needs a box launched with `video`.

| Parameter | Type | Effect |
| --- | --- | --- |
| `op`* | `start` \| `stop` \| `status` | `stop` ends the recording and gives the file path. |
| `fps` | integer | For `start`: frames per second, 1 to 60. Default 12. |

### `cursor`

Return the pointer position. A screenshot does not show the pointer, and a click with no point occurs at the pointer.

## Mouse and keyboard

These tools use screen coordinates. On a web page, use the [page tools](#pages) instead.

| Tool | Parameters | Effect |
| --- | --- | --- |
| `click` | `x`*, `y`*, `button`, `double`, `held`, `motion`, `seed` | Click at a point from the most recent screenshot. `held` keeps modifier keys down during the click. |
| `drag` | `from_x`*, `from_y`*, `to_x`*, `to_y`*, `button`, `held`, `motion`, `seed` | Press at one point and release at another. |
| `scroll` | `x`*, `y`*, `dy`*, `dx` | Turn the wheel at a point, in notches. Positive `dy` is down, positive `dx` is right. |
| `mouse_down` | `x`, `y`, `button`, `hold_seconds`, `motion`, `seed` | Press a button and keep it down. |
| `mouse_up` | `x`, `y`, `button` | Release the button. |
| `type_text` | `text`*, `delay_ms` | Type into the focused element. `delay_ms` adds time between keys, for inputs that lose characters. 30 to 50 is usually enough. |
| `press_key` | `chord`*, `then`, `held` | Press a key or combination, such as `enter` or `ctrl+a`. `then` presses more keys while `held` modifiers stay down. |
| `key_down` | `key`*, `hold_seconds` | Press one key and keep it down. |
| `key_up` | `key`* | Release the key. |

The server releases a button or key after `hold_seconds` (default 10, maximum 60), or when a person takes control.

`press_key` accepts common key names: `esc`, `return`, `pgdn`, `cmd`, and `win` all work. `chord: "tab", then: ["tab"], held: ["alt"]` goes to the third window. Two separate `alt+tab` presses go only to the second.

## Pages

These tools find elements by `query`, so they do not depend on coordinates. Each action result ends with the controls that appeared, changed, or went away, by reference.

### Open and navigate

| Tool | Parameters | Effect |
| --- | --- | --- |
| `open_url` | `url`*, `target`, `label` | Open a URL. `target: blank` (default) opens a new tab and returns its ID. `target: current` navigates the front page. `label` gives the tab a name that `tab` accepts. |
| `tabs` | `op`, `tab` | `list` (default), `switch`, or `close`. |
| `history` | `go`* | `back`, `forward`, or `reload`. Back keeps the page state. Opening the old URL again does not. |
| `scroll_page` | `to`, `dx`, `dy`, `query` | Move the page, or a scrollable element, in pixels. `to` is `by` (default), `top`, or `bottom`. The result gives the position. The same position two times means that no more content loads. |

### Read

| Tool | Parameters | Effect |
| --- | --- | --- |
| `read_page` | `format`, `limit` | Read the page as `markdown` (default), `text`, or `raw` HTML, with link addresses. |
| `snapshot` | `scope`, `limit`, `urls`, `delta`, `quiet_ms` | List all controls on the page, each with a reference such as `@e12`. See below. |
| `find` | `query`, `role`, `exact`, `scroll`, `limit` | Find elements. Each match gives its role, state, position, and a selector that names only that element. |
| `evaluate` | `expression`*, `timeout_ms`, `limit` | Run JavaScript and return its value. `await` works. A DOM node returns `{}`. |
| `console` | `errors`, `clear`, `limit` | Show what the page logged: console calls, uncaught errors, and failed requests. Default 200 lines. |
| `page_screenshot` | `full`, `format`, `quality`, `annotate` | Capture the page with no window frame or address bar. See below. |
| `page_pdf` | `path`*, `landscape`, `no_background` | Print the page to a PDF file in the box. |

`snapshot`:

- An element keeps its reference across snapshots of the same page. A navigation clears all references, so take a new snapshot after one.
- `delta` returns only what changed since the last snapshot with the same `scope`.
- `quiet_ms` waits until the page does not change for this time.
- `urls` adds link addresses.

`find`:

- `role` is one of `button`, `link`, `textbox`, `checkbox`, `radio`, `combobox`, `option`, `heading`, `image`, `tab`, or `dialog`. It matches by function, so `button` also finds `<div role=button>`.
- `exact` matches all of the text, not part of it.
- `scroll` brings the best match into view first.

`page_screenshot`:

- `full` captures the full scrollable page. It returns JPEG unless you set `format`.
- `quality` is for JPEG, 1 to 100. Default 70.
- `annotate` draws each control's reference from the last snapshot.

### Act

| Tool | Parameters | Effect |
| --- | --- | --- |
| `click_element` | `query`*, `button`, `double`, `new_tab`, `motion`, `seed` | Scroll to an element and click it. If a dialog covers it, the result names the dialog. `new_tab` opens a link in a new tab. |
| `fill_field` | `query`*, `text`* | Type text into a field. The page gets real keystrokes. |
| `focus` | `query`* | Give an element the keyboard focus, with no click. Use it before `type_text`. |
| `check` | `query`*, `on` | Set a checkbox or radio (`on: true`, default), or clear a checkbox (`on: false`). No click occurs if it is already in that state. |
| `dropdown` | `query`*, `op`*, `options` | `list`, `select`, or `deselect` options by text or value. This is the only way to use a native dropdown. |
| `upload_file` | `query`*, `paths`* | Give files in the box to a file input. Put a file in the box with `write_file` first. |
| `hover` | `query`*, `motion`, `seed` | Move the pointer over an element. |
| `drag_element` | `from`*, `to`*, `button`, `motion`, `seed` | Drag one element to another. Both must be in the window. |
| `highlight` | `query`*, `seconds` | Draw a box around an element. Default 3 seconds, maximum 60. |
| `dialog` | `accept`*, `text` | Answer a confirm, prompt, or leave-page dialog. `text` goes into a prompt. |

Field, focus, dropdown, and upload tools follow an HTML label to its control. They do not guess from nearby text. When a match is ambiguous, the tool refuses and names the nearby fields.

While a dialog is open, page tools do not work. The tool that opened the dialog fails and gives its text. Alerts are accepted automatically.

### Wait

#### `wait_for`

Wait until an element appears, or with `gone`, goes away. Use it after an action that loads a page or fetches data.

| Parameter | Type | Effect |
| --- | --- | --- |
| `query` | string | The element. |
| `exact` | boolean | Match all of the text. |
| `gone` | boolean | Wait for the element to go away. |
| `enabled` | boolean | Wait for the element to accept input. |
| `load` | boolean | Wait for the document to load first. |
| `or` | string[] | Other text that stops the wait, such as `sold out`. The result says which text matched. |
| `until` | string | A JavaScript expression. The wait stops when it is truthy. An exception counts as not yet. |
| `quiet_ms` | integer | Wait until the page does not change for this time. 500 is usually enough. |
| `within_ms` | integer | The time limit. |

## Native windows

### `window`

| Parameter | Type | Effect |
| --- | --- | --- |
| `op`* | `list` \| `active` \| `focus` \| `close` \| `arrange` \| `wait` | `list` gives the ID, class, position, and size of each window. `active` gives the window that gets keyboard input. `wait` returns when a window of `class` appears and stops moving. |
| `window` | string | The window ID, for `focus`, `close`, and `arrange`. |
| `how` | `move` \| `size` \| `max` \| `min` \| `restore` | For `arrange`. `move` takes `x` and `y`. `size` takes `width` and `height`. |
| `x`, `y`, `width`, `height` | integer | For `arrange`. |
| `class` | string | For `wait`. |
| `within_ms` | integer | For `wait`. |

### `widget`

Drive a native window by the names of its widgets. Needs a box launched with `accessibility`. A query also matches the label next to a field.

| Parameter | Type | Effect |
| --- | --- | --- |
| `op`* | `find` \| `tree` \| `press` \| `fill` \| `focus` | `find` gives the role, name, and rectangle of each match. `tree` gives all widgets. `press` runs the widget's action. `fill` sets a field. `focus` gives it the keyboard. |
| `query` | string | Text on or next to the widget. Necessary for `find`, `press`, `fill`, and `focus`. |
| `role` | string | The toolkit's role, such as `push button` or `text`. |
| `exact` | boolean | Match all of the text. |
| `app` | string | Only this application. |
| `action` | string | For `press`, when the widget has more than one action. `find` lists them. |
| `value` | string | For `fill`. |
| `depth` | integer | For `tree`. |
| `limit` | integer | Maximum matches. |

`press` sends no pointer event, so it works on a covered widget. If the application must see the pointer, use `find` to get the rectangle and then `click`.

### `open_app`

| Parameter | Type | Effect |
| --- | --- | --- |
| `app`* | string | A name the box was launched with, or one that `install_app` installed. |
| `args` | string[] | Arguments for the application, such as a file to open. |

Opens the application and returns when it is ready for input.

### `install_app`

| Parameter | Type | Effect |
| --- | --- | --- |
| `apps`* | string[] | Names from `list_apps`, such as `gimp` or `vscode`. |

Installs the applications into the running box with the package manager, so that `open_app` can open them. It takes from seconds to a minute, and the box must reach the package mirrors.

## Files and commands

| Tool | Parameters | Effect |
| --- | --- | --- |
| `list_files` | `path` | List a directory. Default `/`. |
| `read_file` | `path`* | Read a UTF-8 file. Other files are refused. |
| `write_file` | `path`*, `text`* | Write a text file. It replaces the file. |
| `grep` | `pattern`*, `path`*, `include`, `ignore_case`, `limit` | Search in files under a directory with a basic regular expression. `include` filters file names, such as `*.conf`. |
| `glob` | `pattern`*, `path`, `limit` | Find files by name, such as `*.log`. A pattern with `/` matches the full path. |
| `run_command` | `command`* | Run a command in the box. `command` is an argument list, such as `["ls", "-la", "/tmp"]`. |
| `clipboard` | `text`, `selection` | Read the clipboard, or set it when `text` is given. `selection` is `clipboard` (default) or `primary`. |

`grep` and `glob` stop at 200 results and tell you when they stopped.

## Browser state

| Tool | Parameters | Effect |
| --- | --- | --- |
| `save_state` | `name`*, `origins`, `session_storage`, `indexed_db` | Save cookies and storage on the server under a name, until the server restarts. With no `origins`, the origins of the open tabs. |
| `load_state` | `name`* | Load saved state into the browser. Open the site after it. |
| `cookies` | `op`*, `url`, `values`, `cookies`, `curl`, `all` | `list`, `set`, or `clear` cookies. See below. |

`cookies`:

- `list` shows names and sites. It shows values only with `values: true`.
- `set` takes `url` with `cookies`, a list of `{name, value, http_only}`, or takes `curl`, the text from a browser's "Copy as cURL".
- `clear` takes `url` for one site, or `all: true`.

To keep a login after the server restarts, launch boxes with `profile`.

## Human control

| Tool | Effect |
| --- | --- |
| `open_screen` | Show the person the live screen, with buttons to take control and to record. It does not change the box. |
| `screen_status` | Show who controls the screen, whether it is recording, and a new short-lived token for the viewer. |
| `hand_over` | Give the screen to a person. Returns a URL. The agent's input is refused until `reclaim_screen`. |
| `reclaim_screen` | Take the screen back. |

Hosts that support [MCP Apps](https://github.com/modelcontextprotocol/ext-apps) render `ui://holm/screen.html` beside the results of `launch_box`, `open_screen`, and `hand_over`. The page gets a short-lived token under `_meta`. The model does not get it.

## Batches

### `batch`

Run many steps in one call. The box holds the screen for all steps, and one frame comes back at the end.

| Parameter | Type | Effect |
| --- | --- | --- |
| `actions`* | object[] | The steps, in order. Each is `{"tool": "<name>", "arguments": {…}}`, with no `box_id`. |
| `keep_going` | boolean | Continue after a step is refused. Use it for drawings, not for forms. |
| `settle_ms` | integer | Wait this long after the last step, before the frame. |

A step can be any of these tools:

`click`, `move`, `mouse_down`, `mouse_up`, `drag`, `draw`, `scroll`, `type_text`, `press_key`, `key_down`, `key_up`, `dialog`, `wait`, `wait_until_still`, `open_url`, `open_app`, `click_element`, `fill_field`, `focus`, `check`, `dropdown`, `upload_file`, `wait_for`, `hover`, `drag_element`, `history`, `scroll_page`, `evaluate`, `find`, `snapshot`, `read_page`, `page_screenshot`, `screenshot`, `cursor`, `windows`, `wait_for_window`, `window`, `tabs`, `run_command`, `read_file`, `write_file`, `list_files`, `grep`, `glob`, `clipboard`, `record`, `list_apps`

Steps that are only in a batch:

- `move` moves the pointer. `pause_ms` waits after the move. 40 is enough for a drawing program to see each point.
- `draw` takes `through`, a list of `{x, y}`. It makes one press, moves through each point, and makes one release.

A step that reads, such as `find` or `run_command`, returns its result on its own line. A button or key that is still down at the end is released, and the result says so, unless its step gave `hold_seconds`.

Example:

```json
{
  "box_id": "box_…",
  "actions": [
    { "tool": "open_url", "arguments": { "url": "https://example.com/login" } },
    { "tool": "fill_field", "arguments": { "query": "Email", "text": "me@example.com" } },
    { "tool": "fill_field", "arguments": { "query": "Password", "text": "…" } },
    { "tool": "click_element", "arguments": { "query": "Sign in" } },
    { "tool": "wait_for", "arguments": { "query": "Dashboard", "or": ["Invalid password"] } }
  ]
}
```

## Content boundaries

Set `HOLM_CONTENT_BOUNDARIES=1` on the process that serves MCP. Then `read_page`, `snapshot`, `find`, `evaluate`, and `console` put page text between two markers with a nonce that the page cannot know. The model can then tell page text from tool text.
