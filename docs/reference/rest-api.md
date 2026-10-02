# REST API reference

`computerd` serves the REST API under `/v1`. The wire types are in the [`computer-api`](../../crates/computer-api) crate, and [`computer-client`](../../crates/computer-client) is a Rust client for it.

To start the server and set a token, see [The server](../concepts/server.md).

## Conventions

**Base URL.** `http://127.0.0.1:8080` unless `COMPUTER_SERVER_ADDR` changes it.

**Authentication.** When the server has a token, send `Authorization: Bearer <COMPUTER_SERVER_TOKEN>` on each request, or a workspace token or API key (`holm_sk_…`) from a console. See [Workspaces](../concepts/server.md#workspaces). These endpoints need no bearer token:

- `GET /v1/health`
- The viewer socket, which takes a short-lived token in the query
- The `/v1/cdp/{token}/…` proxy, which takes a short-lived token in the path

**Bodies.** Requests and responses are JSON. Request bodies refuse unknown keys, so a misspelled key gives an error.

**Screens.** `{screen}` is the screen number, from `0`.

**Tabs.** `tab` is a tab ID or label, from `GET /v1/boxes/{id}/pages` or an `open_url` action. With no `tab`, the endpoint uses the page at the front.

**Points.** A point is `{"x": 640, "y": 400}`, in device pixels. `(0, 0)` is the top-left corner.

**Idempotency.** `POST /v1/boxes`, `POST …/actions`, and `POST …/fork` accept an `idempotency-key` header. The same key with the same body returns the first result, and does not do the work again. The server keeps keys in memory.

## Errors

All errors have this body:

```json
{ "code": "denied", "message": "…", "retryable": false }
```

| `code` | HTTP status | Meaning |
| --- | --- | --- |
| `bad_request` | 400 | The request is not valid. |
| `unsupported` | 400 | The box or runtime cannot do this. |
| `not_found` | 404 | No such box, runtime, or object. |
| `gone` | 410 | The box was removed. |
| `denied` | 409 | Refused. Usually a person has control of the screen. |
| `screen_unavailable` | 409 | The screen cannot be used now. |
| `failed` | 422 | The action ran and did not succeed. |
| `transport` | 502 | The server could not reach the box. |
| `unavailable` | 503 | The runtime or store is not available. |
| `timeout` | 504 | The action did not finish in time. |
| `internal` | 500 | A server error. |

`retryable` tells you if the same request can succeed later.

## Endpoints

### Health

| Method | Path | Effect |
| --- | --- | --- |
| `GET` | `/v1/health` | Returns `{"ok": true, "service": "computer-server"}`. No token. |

### Boxes

| Method | Path | Effect |
| --- | --- | --- |
| `GET` | `/v1/boxes` | List boxes. |
| `POST` | `/v1/boxes` | Create a box. See [Create a box](#create-a-box). |
| `GET` | `/v1/boxes/{id}` | Get one box. |
| `DELETE` | `/v1/boxes/{id}` | Remove a box. Needs the header `x-computer-confirm-delete: true`. The server keeps the record of a removed box for its history: the box leaves the list, and each later call to it answers `410`. |
| `POST` | `/v1/boxes/{id}/pause` | Pause the box. |
| `POST` | `/v1/boxes/{id}/resume` | Resume a paused or stopped box. |
| `POST` | `/v1/boxes/{id}/stop` | Stop the box and keep its files. |
| `POST` | `/v1/boxes/{id}/apps` | Install catalog applications in a running box. Body: `{"apps": ["gimp"]}`. |
| `POST` | `/v1/boxes/{id}/fork` | Make a new box from the trace. See [Fork](#fork). |
| `GET` | `/v1/catalog` | List the application names in the catalog. |

A box in a response:

```json
{
  "id": "box_9cf78792…",
  "runtime": "docker",
  "spec_digest": "…",
  "state": "ready",
  "screens": 1,
  "width": 1280,
  "height": 800,
  "viewer_url": "…",
  "devtools_url": "…",
  "created_at_ms": 1790000000000,
  "expires_at_ms": 1790003600000,
  "owner": "ws_…",
  "spec": { "desktop": { … }, "apps": { … }, "policy": { … } },
  "placement": { "runtime": "docker", … }
}
```

`state` is `ready`, `paused`, `stopped`, `unreachable`, `gone`, `starting`, or `failed`. `reason` is present when the state needs an explanation. When the server queues its jobs (`COMPUTER_SERVER_JOBS=queue`), `POST /v1/boxes` answers `202` with a box in the `starting` state. Read the box until it is `ready` or `failed`. `POST /v1/boxes/{id}/fork` answers `202` in the same way, with an empty replay report. A box that is `starting` or `failed` refuses other calls with `409`, and a `failed` box stays until it is deleted. `owner` is the workspace that launched the box, and is absent for a box launched with the server token. `spec` and `placement` are what the box was launched with, so the same box can be launched again.

### Screen

| Method | Path | Effect |
| --- | --- | --- |
| `POST` | `/v1/boxes/{id}/screens/{screen}/actions` | Run a batch of actions. See [Actions](#actions). |
| `GET` | `/v1/boxes/{id}/screens/{screen}/frame` | Get a screenshot. |
| `GET` | `/v1/boxes/{id}/screens/{screen}/cursor` | Get the pointer position. |
| `GET` | `/v1/boxes/{id}/screens/{screen}/clipboard` | Read the clipboard. Query: `selection` = `clipboard` (default) or `primary`. |
| `PUT` | `/v1/boxes/{id}/screens/{screen}/clipboard` | Set the clipboard. Body: `{"text": "…", "selection": "clipboard"}`. |
| `GET` | `/v1/boxes/{id}/screens/{screen}/recording` | Get the recording state. |
| `POST` | `/v1/boxes/{id}/screens/{screen}/recording` | Start a recording. Body: `{"fps": 12}`. Needs the `video` feature. |
| `DELETE` | `/v1/boxes/{id}/screens/{screen}/recording` | Stop the recording. Returns the file path in the box. |
| `POST` | `/v1/boxes/{id}/screens/{screen}/desktop/node` | Act on native widgets. See [Native widgets](#native-widgets). |

`frame` query parameters:

| Parameter | Effect |
| --- | --- |
| `have` | The hash of the last frame. If the screen did not change, the response has `unchanged: true` and no image. |
| `window` | Capture one window. |
| `x`, `y`, `width`, `height` | Capture a rectangle. |
| `scale` | Percent of full size, 1 to 400. |

A frame in a response:

```json
{ "hash": "…", "unchanged": false, "png_base64": "…" }
```

### Windows

| Method | Path | Effect |
| --- | --- | --- |
| `GET` | `/v1/boxes/{id}/screens/{screen}/windows` | List windows. |
| `GET` | `/v1/boxes/{id}/screens/{screen}/windows/active` | Get the window that receives keyboard input. |
| `POST` | `/v1/boxes/{id}/screens/{screen}/windows/wait` | Wait for a window. Body: `{"class": "gimp", "within_ms": 30000}`. |
| `POST` | `/v1/boxes/{id}/screens/{screen}/windows/{window}/focus` | Bring a window to the front. |
| `POST` | `/v1/boxes/{id}/screens/{screen}/windows/{window}/arrange` | Move or resize a window. See below. |
| `DELETE` | `/v1/boxes/{id}/screens/{screen}/windows/{window}` | Close a window. |

`arrange` bodies:

```json
{ "how": "at", "to": { "x": 0, "y": 0 } }
{ "how": "size", "width": 800, "height": 600 }
{ "how": "maximise" }
{ "how": "minimise" }
{ "how": "restore" }
```

### Human control

| Method | Path | Effect |
| --- | --- | --- |
| `POST` | `/v1/boxes/{id}/screens/{screen}/takeover` | Give the screen to a person. Body: `{"shared": false}`. Returns `{url, exclusive, screen}`. |
| `DELETE` | `/v1/boxes/{id}/screens/{screen}/takeover` | Take the screen back. |
| `GET` | `/v1/boxes/{id}/screens/{screen}/viewers` | Returns `{watching, driving, person_driving, taken_over}`. |
| `POST` | `/v1/boxes/{id}/screens/{screen}/viewer/ticket` | Make a short-lived token for the viewer socket. Returns `{ticket, expires_at_ms}`. Valid for 15 minutes. For a box with a signed viewer, the reply also has `view_socket` and, for a member or higher, `control_socket`: WebSocket URLs that go to the box directly. |
| `GET` | `/v1/boxes/{id}/screens/{screen}/viewer/socket` | The viewer WebSocket, through the server. Query: `ticket`, and `mode` = `view` (default) or `control`. No bearer token. |

See [Human control](../concepts/human-control.md).

### Files and commands

| Method | Path | Effect |
| --- | --- | --- |
| `POST` | `/v1/boxes/{id}/exec` | Run a command. Body: `{"argv": ["ls", "-la"], "timeout_ms": 10000}`. Returns `{code, stdout, stderr, timed_out}`. Maximum 10 minutes. |
| `GET` | `/v1/boxes/{id}/files` | Read a file. Query: `path`. Returns `{path, contents_base64}`. |
| `PUT` | `/v1/boxes/{id}/files` | Write a file. Body: `{"path": "/tmp/a.txt", "contents_base64": "…"}`. |
| `GET` | `/v1/boxes/{id}/files/list` | List a directory. Query: `path`. |
| `POST` | `/v1/boxes/{id}/files/grep` | Search in files. Body: `{"pattern": "…", "path": "/etc", "include": "*.conf", "ignore_case": false, "limit": 200}`. |
| `GET` | `/v1/boxes/{id}/files/glob` | Find files by name. Query: `pattern`, `path`, `limit`. |

`grep` and `glob` return `cut: true` when they stop at the limit.

### Pages

These endpoints use Chrome DevTools. On remote runtimes, see [Control modes](../concepts/control-modes.md#browser-mode-on-remote-runtimes).

| Method | Path | Effect |
| --- | --- | --- |
| `GET` | `/v1/boxes/{id}/pages` | List tabs. |
| `DELETE` | `/v1/boxes/{id}/pages/{tab}` | Close a tab. |
| `POST` | `/v1/boxes/{id}/pages/{tab}/focus` | Bring a tab to the front. |
| `GET` | `/v1/boxes/{id}/page` | Read the page. Query: `format` (`markdown`, `text`, `raw`), `limit`, `max_links`, `tab`. |
| `GET` | `/v1/boxes/{id}/page/find` | Find elements. Query: `q`, `limit`, `scroll`, `exact`, `tab`. |
| `GET` | `/v1/boxes/{id}/page/snapshot` | List the controls on the page. Query: `scope`, `limit`, `delta`, `quiet_ms`, `tab`. |
| `POST` | `/v1/boxes/{id}/page/element` | Act on an element. Query: `settle_ms`, `tab`. See [Element operations](#element-operations). |
| `POST` | `/v1/boxes/{id}/page/evaluate` | Run JavaScript. Query: `tab`. Body: `{"expression": "document.title", "timeout_ms": 5000, "limit": 2000}`. |
| `POST` | `/v1/boxes/{id}/page/screenshot` | Capture the page. Body: `{"full": false, "format": "png", "quality": 70, "annotate": false, "tab": null}`. |
| `POST` | `/v1/boxes/{id}/page/pdf` | Print the page. Body: `{"landscape": false, "no_background": false, "path": "/tmp/page.pdf"}`. |
| `POST` | `/v1/boxes/{id}/page/console` | Read the console. Body: `{"errors": false, "clear": false, "limit": 200}`. |

### Browser state

| Method | Path | Effect |
| --- | --- | --- |
| `POST` | `/v1/boxes/{id}/state/save` | Save cookies and storage. Body: `{"origins": [], "name": "work", "session_storage": false, "indexed_db": false, "no_local_storage": false}`. With no `name`, the response has the state in `session_json`. |
| `POST` | `/v1/boxes/{id}/state/load` | Load state. Body: `{"name": "work"}`, or `{"session_json": "…"}` for state from a file. |
| `GET` | `/v1/states` | List saved names. |
| `DELETE` | `/v1/states/{name}` | Remove a saved name. |
| `GET` | `/v1/boxes/{id}/cookies` | List cookies. Query: `url`. |
| `POST` | `/v1/boxes/{id}/cookies` | Set cookies. See below. |
| `DELETE` | `/v1/boxes/{id}/cookies` | Clear cookies. Query: `url`, or none for all. |

Set cookies:

```json
{
  "url": "https://example.com",
  "cookies": [
    { "name": "session", "value": "…", "http_only": true, "secure": true, "same_site": "Lax", "expires": 1790000000 }
  ]
}
```

`domain` and `path` are also accepted on each cookie.

### Chrome DevTools

| Method | Path | Effect |
| --- | --- | --- |
| `POST` | `/v1/boxes/{id}/cdp` | Make a CDP address with a short-lived token. Query: `ttl_secs` (default one hour, maximum 24 hours). Returns `{url, ws_url, expires_at_ms}`. |
| any | `/v1/cdp/{token}/json`, `/v1/cdp/{token}/json/…` | The DevTools HTTP endpoints, through the server. No bearer token. |
| `GET` | `/v1/cdp/{token}/devtools/…` | The DevTools WebSocket, through the server. No bearer token. |

Give `url` to Playwright `connectOverCDP` or browser-use `cdp_url`. Give `ws_url` to a library that needs the WebSocket address.

### History

| Method | Path | Effect |
| --- | --- | --- |
| `GET` | `/v1/boxes/{id}/trace` | Read the trace. Query: `after` (sequence number), `limit` (maximum 500). |
| `GET` | `/v1/boxes/{id}/trace/frames/{hash}` | Get a frame that the trace refers to. |

### Runtimes and images

| Method | Path | Effect |
| --- | --- | --- |
| `GET` | `/v1/runtimes` | List runtimes. |
| `GET` | `/v1/runtimes/{name}` | Get one runtime, with its capabilities under `can`. |
| `POST` | `/v1/runtimes` | Add a remote vendor. See below. |
| `PATCH` | `/v1/runtimes/{name}` | Change fields or secrets. Body: `{"fields": {…}, "secrets": {"api_key": "…"}}`. |
| `DELETE` | `/v1/runtimes/{name}` | Remove a vendor. Refused while a box uses it. |
| `POST` | `/v1/runtimes/{name}/image` | Build the image for a spec before a box needs it. Body: `{"spec": {…}}`. Returns `{runtime, image, spec_digest}`. When the server queues its jobs, it answers `202` with `state: "building"` and an empty `image`. |
| `GET` | `/v1/runtimes/{name}/images` | List the images built for a runtime. |
| `GET` | `/v1/runtimes/{name}/images/{digest}` | Read one image. `state` is `building` or `failed` (with `reason`), and is absent when the image is built. |
| `DELETE` | `/v1/runtimes/{name}/images/{digest}` | Remove an image. Refused while a box uses it. |
| `GET` | `/v1/images` | List all images the server built. |

Add a vendor:

```json
{ "name": "cloud", "provider": "e2b", "fields": { "max_lifetime_secs": 86400 }, "secrets": { "api_key": "…" } }
```

The server encrypts secrets before it stores them and never returns them. See [Runtimes](../concepts/runtimes.md#remote-runtimes).

### Events

The server keeps a log of what happens to boxes, screens and images, for usage and for webhooks.

| Method | Path | Effect |
| --- | --- | --- |
| `GET` | `/v1/events` | Read events in order. Query: `after` (the `next` of the last read, default 0) and `limit` (default 200, maximum 1000). Returns `{events, next, more}`. |

Each event has `seq`, `at_ms`, `kind`, and, when they apply, `owner`, `box_id`, `runtime` and `data`. A workspace reads its own events. The server token reads all of them. An event shows about two seconds after it happens, and the server keeps events for 7 days.

| Kind | When |
| --- | --- |
| `box.created` | A box was launched. `box.ready` follows it. |
| `box.ready` | A box is ready, after a launch, a resume or a start. |
| `box.paused`, `box.stopped` | A box was paused or stopped. |
| `box.removed` | A box was deleted, or it passed its deadline. |
| `box.unreachable` | The runtime no longer has the box. |
| `box.failed` | A queued box did not start. `data.why` says why. |
| `screen.taken_over`, `screen.given_back` | A person took a screen, or it was taken back. |
| `image.built`, `image.removed` | An image was built or removed on a runtime. |

### Scheduled work

For a server that cannot keep a loop alive, such as a serverless function. Set `COMPUTER_SERVER_SCHEDULE=external`, and call these routes from a scheduler. Each accepts `GET` and `POST`, with the server token or `CRON_SECRET` as the bearer token.

| Method | Path | Effect |
| --- | --- | --- |
| `GET`, `POST` | `/v1/jobs/reap` | Remove the boxes that are past their deadline. Returns `{removed}`. Call it each minute. |
| `GET`, `POST` | `/v1/jobs/prune` | Remove old frames, trace entries and expired notes. Returns `{frames, entries, boxes}`. Call it each hour. |
| `GET`, `POST` | `/v1/jobs/run` | Do the queued launches, forks and builds. It stops taking new jobs after 50 seconds. Returns `{ran}`. Call it each minute. |

When `COMPUTER_PUBLIC_URL` and `CRON_SECRET` are set, the server calls its own `/v1/jobs/run` when it queues a job, so a job does not wait for the scheduler. On Vercel, the schedule goes in `vercel.json`:

```json
{
  "crons": [
    { "path": "/v1/jobs/run", "schedule": "* * * * *" },
    { "path": "/v1/jobs/reap", "schedule": "* * * * *" },
    { "path": "/v1/jobs/prune", "schedule": "0 * * * *" }
  ]
}
```

### MCP

`/mcp` serves MCP over Streamable HTTP, with the same bearer token. See the [MCP tools reference](mcp-tools.md).

## Create a box

`POST /v1/boxes`

```json
{
  "spec": {
    "desktop": {
      "server": "x11",
      "width": 1280,
      "height": 800,
      "screens": 1,
      "features": ["wide_fonts", "video", "dock", "accessibility", "audio"],
      "packages": ["jq"]
    },
    "policy": { "network": true }
  },
  "placement": {
    "runtime": "docker",
    "memory": "2g",
    "cpus": "2",
    "expires_after_secs": 3600,
    "idle_timeout_secs": 900,
    "profile": "work"
  }
}
```

All fields are optional. `{}` creates a box with the defaults.

`spec.desktop`:

| Field | Values |
| --- | --- |
| `server` | `x11` (default) or `wayland` |
| `width`, `height` | Screen size. Default: the image's size. |
| `screens` | Number of screens. Default 1. |
| `features` | The whole set of `wide_fonts`, `audio`, `video`, `dock`, `x11_apps`, `accessibility`. Left out: `wide_fonts`, `video`, `dock`, `accessibility`, and `x11_apps` on Wayland. `[]` is the bare desktop. |
| `packages` | Apt packages |

`spec.policy`:

| Field | Values |
| --- | --- |
| `network` | `false` removes network access. Default `true`. |
| `auth` | Viewer access: `none` (default), `password`, or `token` |
| `bind` | `loopback` (default) or `any` |
| `advertise` | The host name to put in viewer URLs |

`spec.apps` defines applications that are not in the catalog.

`placement`:

| Field | Effect |
| --- | --- |
| `runtime` | A runtime name. Default: the server's default. |
| `memory`, `cpus` | Limits, such as `"2g"` and `"2"` |
| `expires_after_secs` | Remove the box after this time. Minimum 60. |
| `idle_timeout_secs` | Remove the box after this time with no use. Minimum 60. |
| `persistent` | `true` keeps the files of the box when it stops, so that `POST /v1/boxes/{id}/resume` brings it back. Default `false`. A runtime that cannot do this refuses the box. |
| `profile` | A browser profile name. Container runtimes only. |

The response is the box.

## Actions

`POST /v1/boxes/{id}/screens/{screen}/actions`

A batch holds the screen for all its actions and returns one result.

```json
{
  "actions": [
    { "type": "open_url", "url": "https://example.com" },
    { "type": "wait_still", "settle_ms": 400, "within_ms": 10000 },
    { "type": "click", "at": { "x": 640, "y": 81 } }
  ],
  "want": ["frame", "cursor"],
  "settle_ms": 400,
  "have_frame": "…",
  "keep_going": false
}
```

| Field | Effect |
| --- | --- |
| `actions` | The steps, in order. |
| `want` | `frame` and `cursor` add the final screen and pointer position to the result. |
| `settle_ms` | Wait this long after the last step. |
| `have_frame` | The hash of the last frame. If the screen did not change, the frame has `unchanged: true` and no image. |
| `keep_going` | Continue after a step is refused. Default: stop at the first refusal. |

The result:

| Field | Meaning |
| --- | --- |
| `results` | One result for each step that ran. A step that reads data returns it here. |
| `stopped_at` | The index of the step that stopped the batch, or `null`. |
| `frame`, `cursor` | Present when `want` asks for them. |
| `windows`, `tabs` | The windows and tabs after the batch. |
| `released`, `released_keys` | Buttons and keys that the server released at the end. |
| `holding`, `holding_keys` | Buttons and keys still down, with the time the server will release them. |

### Action types

Each action has a `type`. Pointer actions accept `motion` (`instant`, `smooth`, `human`) and `seed`. `button` is `left` (default), `right`, or `middle`. `held` is a list of `shift`, `ctrl`, `alt`, and `super`.

**Input**

| `type` | Fields |
| --- | --- |
| `move` | `to`, `motion`, `seed`, `pause_ms` |
| `click` | `at`, `button`, `held`, `motion`, `seed` |
| `double_click` | `at`, `button`, `motion`, `seed` |
| `drag` | `from`, `to`, `button`, `held`, `motion`, `seed` |
| `path` | `through` (list of points), `button`, `held`, `motion`, `seed` |
| `mouse_down` | `at`, `button`, `motion`, `seed`, `hold_ms` |
| `mouse_up` | `at`, `button` |
| `scroll` | `at`, `dx`, `dy` (notches; positive is down and right) |
| `type` | `text`, `delay_ms` |
| `press` | `chord`, `then`, `held` |
| `key_down` | `key`, `hold_ms` |
| `key_up` | `key` |

With no `at`, a click or button action occurs at the pointer. The server releases a button or key after `hold_ms` (default 10 seconds, maximum 60), when the batch ends, or when a person takes control.

**Waits**

| `type` | Fields |
| --- | --- |
| `wait` | `ms` (maximum 30 seconds) |
| `wait_still` | `settle_ms` (default 400), `within_ms` (default 10000). Both stop at 30 seconds. |
| `await_window` | `what: {class, within_ms}` |

**Screen and windows**

| `type` | Fields |
| --- | --- |
| `capture` | `what: {window, region, scale, pointer, tab}` |
| `cursor` | None |
| `windows` | `active` |
| `on_window` | `window`, `what: {"do": "focus" \| "close" \| "arrange", "how": …}` |
| `launch` | `app` (a catalog name), `args` |
| `apps` | None |
| `record` | `what: {"do": "start", "fps": 12}`, `{"do": "stop"}`, or `{"do": "status"}` |
| `clipboard` | `selection`, `text` (sets it when present) |

**Pages**

| `type` | Fields |
| --- | --- |
| `open_url` | `url`, `target` (`blank` or `current`), `label` |
| `tabs` | None |
| `on_tab` | `tab`, `close` |
| `on_page` | `what`: an [element operation](#element-operations) |
| `look` | `what: {query, limit, scroll, exact, role, tab}` |
| `snapshot` | `what: {scope, limit, tab, delta, quiet_ms}` |
| `read` | `what: {format, limit, max_links, tab}` |
| `evaluate` | `what: {expression, timeout_ms, limit}`. `timeout_ms` defaults to 5000 and stops at 30 seconds. |
| `page_shot` | `what: {full, format, quality, tab, annotate}` |
| `dialog` | `accept`, `text` |

**Native widgets**

| `type` | Fields |
| --- | --- |
| `on_node` | `what`: a [widget operation](#native-widgets) |

**Files and commands**

| `type` | Fields |
| --- | --- |
| `exec` | `what: {argv, timeout_ms}` |
| `read_file` | `path` |
| `write_file` | `what: {path, contents_base64}` |
| `list_files` | `path` |
| `grep` | `what: {pattern, path, include, ignore_case, limit}` |
| `glob` | `what: {pattern, path, limit}` |

### Element operations

The body of `POST /v1/boxes/{id}/page/element`, and the `what` of an `on_page` action. Each has an `op`.

| `op` | Fields |
| --- | --- |
| `click` | `query`, `button`, `double`, `new_tab`, `motion`, `seed` |
| `drag` | `from`, `to`, `button`, `motion`, `seed` |
| `fill` | `query`, `text` |
| `focus` | `query` |
| `check` | `query`, `on` |
| `options` | `query` |
| `choose` | `query`, `options`, `drop` (`true` removes the named options) |
| `upload` | `query`, `paths` (paths in the box) |
| `hover` | `query`, `motion`, `seed` |
| `highlight` | `query`, `ms` |
| `wait_for` | `query`, `gone`, `within_ms`, `or`, `exact`, `quiet_ms`, `enabled`, `load`, `until` |
| `history` | `go` (`back`, `forward`, `reload`) |
| `scroll` | `query`, `to` (`by`, `top`, `bottom`), `dx`, `dy` |

```json
{ "op": "fill", "query": "Email", "text": "me@example.com" }
```

The result includes the element, the URL, whether the page navigated, and the controls that changed.

### Native widgets

The body of `POST /v1/boxes/{id}/screens/{screen}/desktop/node`, and the `what` of an `on_node` action. Needs a box with the `accessibility` feature.

| `op` | Fields |
| --- | --- |
| `tree` | `app`, `depth` |
| `find` | `node`, `limit` |
| `focus` | `node` |
| `invoke` | `node`, `action` |
| `set` | `node`, `value` |

`node` is a query: `{"query": "Street", "role": "text"}`.

## Fork

`POST /v1/boxes/{id}/fork`

```json
{ "mode": "replay", "up_to": 40, "placement": { "expires_after_secs": 1800 } }
```

| Field | Effect |
| --- | --- |
| `mode` | `replay` (default). `snapshot` is refused. |
| `up_to` | Repeat the trace up to this sequence number. |
| `placement` | A placement for the new box. Default: the original's. |

The result is `{"box": {…}, "replay": {attempted, ok, stopped_at, truncated, skipped}}`. Each entry in `skipped` has `seq`, `kind`, and `why`.

## Example

```bash
BASE=http://127.0.0.1:8080

BOX=$(curl -fsS "$BASE/v1/boxes" \
  -H 'content-type: application/json' \
  -H 'idempotency-key: create-demo-1' \
  -d '{"spec": {"desktop": {"width": 1280, "height": 800}}, "placement": {"expires_after_secs": 3600}}' |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')

curl -fsS "$BASE/v1/boxes/$BOX/screens/0/actions" \
  -H 'content-type: application/json' \
  -d '{
    "actions": [
      { "type": "open_url", "url": "https://example.com" },
      { "type": "on_page", "what": { "op": "wait_for", "query": "Example Domain" } },
      { "type": "on_page", "what": { "op": "click", "query": "More information" } }
    ],
    "want": ["frame"]
  }'

curl -fsS -X DELETE "$BASE/v1/boxes/$BOX" -H 'x-computer-confirm-delete: true'
```
