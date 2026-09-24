# computer-server

A REST API over `computer` boxes. Create them, drive them, take them away — all of it over HTTP, so a shell script with `curl` and an MCP server built by mapping tools onto endpoints both work with no SDK in between.

```bash
computerd
# From a source checkout:
cargo run --bin computerd
```

The default address is `127.0.0.1:8080`.

## Configuration

Core server settings:

- `COMPUTER_SERVER_ADDR` — listen address; default `127.0.0.1:8080`
- `COMPUTER_SERVER_TOKEN` — bearer token; required outside loopback
- `COMPUTER_PUBLIC_URL` — public origin used by MCP Apps behind a reverse proxy
- `COMPUTER_SERVER_CONFIG` — path to the runtimes file; read at start, a change needs a restart
- `COMPUTER_SERVER_RUNTIMES` — comma-separated host runtimes to offer; default `docker,podman,nerdctl,smolvm`, and only the ones that answer are offered
- `COMPUTER_SERVER_SANDBOXES` — comma-separated remote vendors, such as `e2b`
- `COMPUTER_SERVER_SECRET_KEY` — 32 bytes, base64 or hex, that seal the keys this server stores
- `COMPUTER_SERVER_SECRET_FILE` — a file holding that key instead; the server makes one at first start, readable by its owner only
- `COMPUTER_SERVER_REAP_SECS` — expired-box sweep interval; default 30 seconds

The default store is in memory. Select durable trace and frame storage with:

- `COMPUTER_STORAGE_BACKEND=local` and `COMPUTER_STATE_DIR=/var/lib/computer`
- `COMPUTER_STORAGE_BACKEND=sqlite|postgres` and `COMPUTER_STATE_URL=<database-url>`
- `COMPUTER_STORAGE_BACKEND=s3`, `COMPUTER_S3_ENDPOINT`, `COMPUTER_S3_BUCKET`, `AWS_ACCESS_KEY_ID`, and `AWS_SECRET_ACCESS_KEY`

S3 also accepts `COMPUTER_S3_REGION` and `COMPUTER_S3_PREFIX`. SQLite, PostgreSQL, and S3 must be enabled when the binary is built:

```bash
cargo install --path . --locked --features sqlite,postgres,s3
```

Retention settings:

- `COMPUTER_KEEP_FRAMES_SECS` — default two hours
- `COMPUTER_KEEP_ENTRIES_SECS` — default seven days
- `COMPUTER_PRUNE_SECS` — pruning interval; default one hour

Example remote deployment:

```bash
export COMPUTER_SERVER_ADDR=0.0.0.0:8080
export COMPUTER_SERVER_TOKEN="$(openssl rand -hex 32)"
export COMPUTER_PUBLIC_URL=https://boxes.example.com
export COMPUTER_STORAGE_BACKEND=local
export COMPUTER_STATE_DIR=/var/lib/computer
computerd
```

Terminate TLS in a reverse proxy and forward `/v1`, `/mcp`, and WebSocket upgrades to `computerd`.

## The gate

Whoever reaches this API creates boxes, drives them, reads their frames and runs commands inside them, so an address it answers on is worth more than any single viewer URL behind it.

The rule is the engine's own. On loopback it opens without a token, which is what a box on a laptop has always been. Bound anywhere else it needs one, and says so at startup rather than on the first unauthenticated request:

```
$ COMPUTER_SERVER_ADDR=0.0.0.0:8080 computerd
Error: 0.0.0.0:8080 can be reached from off this host and COMPUTER_SERVER_TOKEN
is not set. Whoever reaches this API can create boxes, drive them and run
commands inside them. Set a token, or bind to 127.0.0.1.
```

```bash
export COMPUTER_SERVER_TOKEN=$(openssl rand -hex 32)
curl -s localhost:8080/v1/boxes -H "Authorization: Bearer $COMPUTER_SERVER_TOKEN"
```

The token is a `computer::Secret`, so it is refused under 16 characters, and it has no `Display` and no `Serialize` — it cannot reach a log by accident. Comparison is constant-time. `/v1/health` answers without one, since a load balancer has no token and a refusal would tell whoever asked the same thing.

This gates the API. The viewer and control URLs it hands back carry their own credentials, set by `policy.auth` in the spec.

## Create a box

A box is described by a **spec** — what desktop is wanted — and placed by a **placement** — where it runs and for how long. They are separate because two identical desktops that differ only in a memory limit are one desktop, and `spec_digest` is what says so. Both live in `computer-types`, which describes a desktop and how to address one without knowing an API exists.

`width`, `height` and `screens` are optional. A spec that leaves them open takes whatever the image gives, and is a different spec from one that pins them.

```bash
curl -s localhost:8080/v1/boxes -H 'content-type: application/json' \
  -H 'idempotency-key: launch-1' -d '{
    "spec": {
      "desktop": { "width": 1280, "height": 800, "features": ["wide_fonts"] },
      "policy":  { "network": true }
    },
    "placement": { "memory": "2g", "expires_after_secs": 3600 }
  }'
```

Unknown keys in a spec are refused rather than ignored: a misspelled key that is quietly dropped hands back a box missing the thing it was misspelled for.

`"runtime": "docker"` in the placement names one of the runtimes below. A name this server does not have is refused with the names it has, and the box lands on the default when the placement names none.

`"profile": "work"` in the placement keeps the browser's logins, cookies and history in the volume `computer-profile-work` after the box is gone, and the next box given the name starts with them. A profile that another box holds, running or stopped, is refused with that box's name.

## Where a box runs

A runtime is described twice: by its **place**, host or remote, and by the **environment** it runs a box in — a container, a microVM or a VM, with whatever its provider knows about it.

```bash
curl -s localhost:8080/v1/runtimes | jq '.runtimes[] | {name, place, environment}'
```

```json
{
  "name": "docker",
  "place": "host",
  "environment": { "kind": "container", "info": { "engine": "docker 27.3.1", "default_runtime": "runc", "storage": "overlay2" } }
}
```

Host runtimes are found at start, each offered under the name of the program that runs it when it answers: docker, podman and nerdctl put a box in a container, and **smolvm** puts one in a microVM on libkrun — Hypervisor.framework on macOS, KVM on Linux. A box there gets a kernel of its own rather than a namespace, and its ports are forwarded to loopback exactly as an engine publishes them. Tuning an engine stays with the engine — `DOCKER_HOST` and `docker context` point Docker at another host, `default-runtime` in `daemon.json` puts its containers on gVisor or Kata.

What an engine cannot say goes in the file at `COMPUTER_SERVER_CONFIG`, and only there, because every field names a program, a host or an engine of this server's:

```toml
default = "hardened"

[runtimes.docker]
memory = "4g"

[runtimes.smolvm]
program = "/opt/smolvm/bin/smolvm"
memory = "4g"

[runtimes.hardened]
provider = "docker"
isolation = "runsc"

[runtimes.gpu-host]
provider = "docker"
context = "gpu-1"

[runtimes.podman]
enabled = false
```

A table with no `provider` tunes the engine of that name that was found; with one, it offers the same engine again under another name. `memory` and `cpus` are a default for every box on that runtime, and a placement still overrides them. `isolation` is the OCI runtime each box is started on, `--runtime` per box rather than one setting for the whole engine. `context` is a Docker context made with `docker context create`, which keeps its own TLS and SSH settings.

**A box lives an hour**, on every runtime, unless it says otherwise. `lifetime` changes what a box gets when its placement names no `expires_after_secs`, and `max_lifetime` changes the most one may ask for; a placement above the cap is refused with both numbers and told which field to raise. Written as `24h`, `90m`, `3600s`, or a whole number of seconds. A vendor named in `COMPUTER_SERVER_SANDBOXES` takes these two as well, so an account that keeps a box for a day says so rather than being guessed at:

```toml
[runtimes.e2b]
max_lifetime = "24h"
lifetime = "4h"
```

A vendor can also be added while the server runs, and is then kept in the store:

```bash
printf '%s' "$E2B_API_KEY" | computer runtime add cloud --provider e2b --api-key
computer runtime ls
computer runtime set cloud --field max_lifetime_secs=86400
computer runtime rm cloud
```

The key never goes on the command line, where `ps` and the shell's history keep it: it arrives on stdin, or the CLI reads a variable named with `--api-key-env` and sends the value.

What the server does with it:

- **It is sealed before it is stored.** ChaCha20-Poly1305 under the server key, with the runtime's name and provider bound in, so a row copied into another runtime's place does not open. A copy of the database is not a copy of the account. With no server key set, storing a secret is refused rather than written in the clear.
- **It is written, never read back.** No route returns a key; `GET /v1/runtimes` lists the names only, such as `["api_key"]`.
- **Only vendors.** The API cannot name a host engine, a program or a socket, so it cannot make this server run something.
- **An endpoint added this way must be reachable from outside.** `https`, and not loopback, a private address or a `.internal` name. The file may name anything, because an operator wrote it.
- **One namespace.** A name the file, the environment or a host engine already has is refused, and those runtimes are changed where they are written rather than over the API.
- **A new key takes the running boxes with it.** `PATCH` rebuilds the runtime and attaches its boxes again, so rotating a key does not strand them.
- **Removing one is refused while a box record names it.**

A stored runtime whose key this server cannot open — a lost or changed server key — is listed with `state: unavailable` and the reason, rather than disappearing.

A microVM runtime takes `program`, for a hypervisor installed away from the path, and the same `memory`, `cpus`, `lifetime` and `max_lifetime` as an engine. It cannot pause or stop a box, and it refuses a named browser profile, which needs a volume; `GET /v1/runtimes/smolvm` says so.

`GET /v1/runtimes/{name}` says what one can do: whether it pauses, whether it stops, how a port is reached, and whether memory and cpus are set when a box is created or when its image is built. A placement asking for something the runtime cannot do is refused before anything starts.

## Drive it

An agent's step is several actions and one look, so that is the request. The frame the actions produced comes back in the same response.

```bash
curl -s localhost:8080/v1/boxes/$BOX/screens/0/actions \
  -H 'content-type: application/json' -d '{
    "actions": [
      { "type": "move",  "to": { "x": 640, "y": 400 } },
      { "type": "click" },
      { "type": "type",  "text": "driven over rest" }
    ],
    "settle_ms": 500,
    "want": ["frame", "cursor"],
    "have_frame": "<the hash you already hold>"
  }'
```

Three things worth knowing:

- **A batch stops at its first failure.** A click that follows a move which failed lands wherever the pointer was, and the frame afterwards looks like it worked. `stopped_at` names the action that ended the run, and everything after it was not attempted.
- **`have_frame` costs nothing when nothing moved.** Frames are named by their contents. A screen that has not changed answers `"unchanged": true` and carries no picture.
- **Send an `idempotency-key` on anything that acts.** A retried click is a double click, which on a real interface opens the file rather than selecting it. A key is bound to the request it first arrived on, so use a fresh one per operation: the same key on a different body or a different endpoint answers `409` rather than handing back the first request's reply for work that never happened.

## A press that waits before it lets go

`mouse_down` and `mouse_up` press a button and let it go as separate actions, for what a `drag` cannot say: hold, wait for something, then release. A `move` between them drags.

```json
{ "actions": [
    { "type": "mouse_down", "at": { "x": 400, "y": 400 } },
    { "type": "on_page", "what": { "op": "wait_for", "query": "Drop here" } },
    { "type": "move", "to": { "x": 900, "y": 600 }, "pause_ms": 40 },
    { "type": "mouse_up" }
] }
```

`hold_ms` on `mouse_down` keeps the button down after the batch ends, which is how `computer mouse <box> down` and `up` are two commands. Nothing holds the screen between two calls, so the server lets the button go itself: after `hold_ms`, 60 seconds at most; when a person takes the screen over; before the box is paused; and when a server takes the box back after a restart, which lost the timer. `holding` names the button and when it will be let go. Without `hold_ms` a button never outlives its batch.

`key_down` and `key_up` do the same for one key: `{ "type": "key_down", "key": "shift" }`, then the clicks that extend a selection, then `key_up`. `key` is one key, not a combination. It takes `hold_ms` as `mouse_down` does, the server lets it go at the same four moments, and `released_keys` and `holding_keys` name it. When a server takes a box back after a restart it lets every key go, because it no longer knows which were down.

`pause_ms` waits after the move, a second at most. A drawing program reads the pointer at intervals and merges moves that arrive faster than it reads: without a pause GIMP joined the two ends of a stroke and never saw the corner between them. 40 is enough.

Both take `at` and `button`, or act where the pointer is. They are safe only inside one batch, which holds the screen for the whole run. A button still down when the batch ends is let go, even when a step failed or a person took the screen over, and `released` names it. The trace records that release, so a fork replays it. X11 keeps a button down by itself. On Wayland a virtual pointer lives only as long as its client, so each screen keeps one `computer-pointer serve` for its whole life and every gesture goes through it.

## A whole form in one request

Page operations are also actions, so a form is one batch rather than a call per field. The screen lock is held across the whole of it, and one frame comes back instead of one per step.

```bash
curl -s localhost:8080/v1/boxes/$BOX/screens/0/actions \
  -H 'content-type: application/json' -d '{
    "actions": [
      { "type": "open_url", "url": "https://example.com/order" },
      { "type": "on_page", "what": { "op": "fill",     "query": "name", "text": "…" } },
      { "type": "on_page", "what": { "op": "click",    "query": "Continue" } },
      { "type": "on_page", "what": { "op": "wait_for", "query": "Details" } },
      { "type": "on_page", "what": { "op": "choose",   "query": "size", "option": "Large" } },
      { "type": "on_page", "what": { "op": "upload",   "query": "spec", "paths": ["/tmp/spec.pdf"] } }
    ],
    "want": ["frame"]
  }'
```

The page is resolved when the first `on_page` asks rather than up front, and dropped again whenever `open_url` runs: that raises a new tab, so a handle taken earlier would address the one it replaced.

Tabs beyond the newest twelve are closed as new ones open — never the one on screen. A person opening a link expects a new tab, but a program doing it fifty times leaves fifty behind and a browser holding them all gets slower at everything.

## Drive a native window

A page has DevTools behind it. A file dialog, a settings panel or an installer has nothing, and a coordinate worked out from a screenshot is the only other way in. The accessibility tree is what the toolkit publishes about its own widgets.

```bash
curl -s localhost:8080/v1/boxes/$BOX/screens/0/desktop/node \
  -H 'content-type: application/json' \
  -d '{ "op": "find", "node": { "query": "Street" } }'
```

`find` answers with each match's role, name, actions and rectangle, best first. `tree` reads everything an application publishes, `focus` gives one the keyboard, `invoke` runs a widget's own action and `set` assigns a value. These are also actions, so a native form is one batch: `{ "type": "on_node", "what": {...} }`.

A query matches the words beside a widget as well as its own name. A GTK entry has no name — "Street" is a separate label next to it — so a search of names alone would find every button and no field.

`invoke` is not a click. No pointer moves, so it reaches a widget that is covered or scrolled out of view, and an application watching the pointer sees nothing of it. Each match carries `at`, so a real click is still one call away. The action name is the toolkit's own: GTK writes `click` where Qt writes `Press`, and `invoke` runs the first unless told otherwise.

Only a box built with `Feature::Accessibility` has a tree. It cannot be turned on afterwards: an application joins the tree only if the bus was there before it drew its first window.

## Open an app

An app is named, not commanded: a caller who could post an argv to a driving endpoint would be running programs through one meant for clicks, and `exec` already does that in the open.

```bash
curl -s localhost:8080/v1/boxes/$BOX/screens/0/actions \
  -H 'content-type: application/json' -d '{
    "actions": [{ "type": "launch", "app": "gimp" }],
    "want": ["frame"]
  }'
```

**The call returns once the app has drawn**, and answers with the window it drew. That is the whole point of it. A window exists well before the program behind it has painted — GIMP maps a splash screen carrying its own class about half a second before the real one, and VS Code maps its window and paints a second later — so a launch that returned on the window appearing would hand back a screen the next click lands wrong on.

`GET /v1/catalog` names what this server knows, and `spec.apps` is what installs one when the box is created. A box that is already running takes one too:

```bash
curl -s localhost:8080/v1/boxes/$BOX/apps -H 'content-type: application/json' \
  -d '{"apps": ["gimp"]}'
```

It adds the app's own apt archive when it needs one, installs it, and writes the launcher, so `open_app` finds it afterwards. It costs the install every time, needs the box to have network, and is gone from a fork, which builds from the spec.

## Read the page

A frame says where to click. It does not say what a page holds: it is a picture of text, shows one viewport of a document that may be far longer, and renders a link as its label rather than its address.

```bash
curl -s "localhost:8080/v1/boxes/$BOX/page?format=markdown&limit=4000&max_links=20"
```

`limit` and `max_links` are optional and cap what comes back — ask for a few hundred characters to decide whether a page is worth reading, and `truncated` says whether that cut anything. Both are ceilings here rather than defaults a caller can raise; `computer::Page::read` has no ceiling, because a library owes its caller none and a deployment owes one to whoever it answers.

`find` answers with each match's position **in the page**, which is what `click_element` and the rest use. `scroll=true` brings the best match into view before measuring: a match below the fold is otherwise described where the window is not looking, and its coordinates address nothing. On a page four thousand pixels tall, the same button reads `y=4011` without it and `y=903` with.

`snapshot` lists every control on the page in document order — links, buttons, fields, dropdowns, checkboxes, tabs, menu items — with the headings between them, and numbers each. The number is kept in the page, so `@e12` is then a query for `find` and for everything `page/element` does, and a second snapshot of the same document hands out the same numbers. `scope` is a query as `find` takes one and narrows the listing to what its first match holds; `total` says how many the page offered when `limit` cut the listing short.

`delta=true` answers only what appeared, changed or left since the last snapshot with the same scope, and a count of what stayed the same, so a page is read in full once and then by difference. The comparison is what each control is and says, not where it sits. The first snapshot of a document has nothing to compare with, so it answers in full with `delta.first`. `quiet_ms` lists the page only once nothing on it has changed for that long, and fails as a wait does when it never settles. Every `page/element` answer carries the same `delta` for what the action made appear, change or leave, so the next step needs no snapshot of its own. Only a snapshot numbers a page: an action on one nobody has snapshotted reports nothing rather than handing out refs nobody has seen, and a ref named there is refused with the reason. A navigation that keeps the document, as a single-page app's `pushState` does, keeps its numbers and its report; one that replaces it starts again.

The pointer actions — `move`, `click`, `double_click` and `drag` in an action batch, and `click`, `hover` and `drag` on `page/element` — take `motion`: `instant`, the default, `smooth` or `human`, and a `seed` for `human`. A path starts where the pointer is, or the middle of the screen where a driver cannot say, and on a page where the page last saw it. `drag` on `page/element` takes `from` and `to` as queries, and both must fit in the window at once.

`format` is `markdown` (the default), `text` or `raw`. Markdown keeps the headings, lists, tables and code a flat rendering loses, and puts each link's address beside its words. `text` is the cheapest answer to "what does this say". `raw` is the document's own HTML — an escape hatch for what the other two do not carry, and megabytes where they are kilobytes.

A page marking `<main>` or `<article>` is read from there, since HTML already defines those as its content. Where neither exists the whole body is read: telling chrome from content without them is a real heuristic, and a wrong one loses the page. Wikipedia marks a `<main>` that holds its own sidebar, so a chrome-heavy page can still spend a reader's budget before its article begins — raise `limit` or go straight to the section you want.

Answers with the visible page's title, URL, rendered text and its links with their `href`s — so a caller reads what is there, and navigates by URL rather than guessing a coordinate for an anchor. Clicking is still how everything that is not a link is reached.

The read is scoped rather than an `evaluate` endpoint: running a caller's JavaScript in the box's browser is a wider door than any tool here needs.

The reading itself is `computer::Page::read`, so a library user gets it without a server and this endpoint is only the HTTP in front of it.

## The rest

| | |
| --- | --- |
| `GET /v1/boxes`, `GET /v1/boxes/{id}` | what is running |
| `DELETE /v1/boxes/{id}` | takes `x-computer-confirm-delete`; the files do not come back |
| `GET /v1/boxes/{id}/screens/{n}/frame?have=` | a screenshot on its own |
| `GET /v1/boxes/{id}/screens/{n}/cursor` | where the pointer is, which a frame never shows |
| `GET`/`PUT` `…/screens/{n}/clipboard` | `?selection=clipboard` or `primary` — they hold different text |
| `POST`/`DELETE` `…/screens/{n}/takeover` | hand the screen to a person, and take it back |
| `GET …/screens/{n}/viewers` | who is watching and who is driving |
| `POST /v1/boxes/{id}/fork` | build it again from its trace |
| `POST /v1/boxes/{id}/cdp?ttl_secs=` | an address another library drives the browser by, with a short-lived token in its path |
| `/v1/cdp/{token}/json/…`, `/v1/cdp/{token}/devtools/…` | the browser's DevTools through this server; the token admits, no bearer |
| `GET /v1/catalog` | the app names a launch can ask for |
| `GET /v1/runtimes`, `GET /v1/runtimes/{name}` | where boxes can be put, what each runs them in, and what each can do |
| `POST /v1/runtimes` | add a vendor: name, provider, fields, secrets. Sealed before it is stored, and checked with the provider before it is kept |
| `PATCH /v1/runtimes/{name}` | change its fields, or give it a new key |
| `DELETE /v1/runtimes/{name}` | refused while a box record names it |
| `GET /v1/boxes/{id}/page?limit=` | the page on screen, as text and links |
| `GET /v1/boxes/{id}/page/find?q=&scroll=` | what matches, best first |
| `GET /v1/boxes/{id}/page/snapshot?scope=&limit=&delta=&quiet_ms=` | every control on the page in order, numbered; or what changed since the last one |
| `POST /v1/boxes/{id}/page/element` | click, fill, dropdown, upload, hover, highlight, drag, wait, history or scroll, by query; `new_tab` on a click opens the link in its own tab |
| `POST /v1/boxes/{id}/state/save`, `…/state/load` | the browser's login, as `session_json` or kept on the server under `name` until it restarts |
| `GET /v1/states`, `DELETE /v1/states/{name}` | the names the server keeps |
| `GET`/`POST`/`DELETE` `/v1/boxes/{id}/cookies?url=` | the browser's cookies: list, set, or clear one site's or every one |
| `POST /v1/boxes/{id}/page/console` | what the page logged since it loaded; `errors` keeps the failures, `clear` empties it |
| `POST /v1/boxes/{id}/page/pdf` | the page printed to PDF, as base64, or into the box at `path` |
| `POST /v1/boxes/{id}/page/screenshot` | the page as the browser drew it; `annotate` draws each snapshot number over its control |
| `GET /v1/boxes/{id}/screens/{n}/windows` | what is on the screen |
| `POST …/windows/{w}/focus`, `DELETE …/windows/{w}` | raise one, close one |
| `POST /v1/boxes/{id}/exec` | one command, one answer |
| `POST /v1/boxes/{id}/apps` | install an app from the catalog into a box that is already running |
| `GET`/`PUT` `/v1/boxes/{id}/files` | base64 in, base64 out |

Every error is the same shape — `code`, `message`, `retryable` — including a body that is not JSON at all, because that is the first error most clients ever see.

The wire types live in [`computer-api`](../computer-api), and [`computer-client`](../computer-client) is a Rust client over them.

## Fork a box

```bash
curl -s -X POST localhost:8080/v1/boxes/$BOX/fork \
  -H 'content-type: application/json' -H 'idempotency-key: fork-1' \
  -d '{ "up_to": 42 }'
```

Launches a box from the same spec and does again what was done to the first one, at roughly the original pace. It reads the *trace*, not the box, so **a box that has been removed can still be forked** — its record outlived it and carries the spec.

The reply counts what happened rather than promising the two boxes match:

```json
{
  "box": { "id": "box_…" },
  "replay": {
    "attempted": 3,
    "ok": 3,
    "truncated": false,
    "skipped": [
      { "seq": 2, "kind": "file_written",
        "why": "the trace records that /tmp/marker.txt was written, not what went into it" }
    ]
  }
}
```

**A replay reconstructs, it does not copy.** The same actions against a page that has since changed, a slower network, or a dialog that appeared this time land somewhere else. Even a faithful replay rarely produces identical pixels — a desktop animates, and a tooltip caught mid-fade is a different picture of the same state. Compare frames yourself if you need to know how close it got; both boxes' traces end with one.

Actions the original was refused are skipped, since replaying them would invent a difference. A replay stops at the first failure and names the source sequence that stopped it, and gives up after three minutes rather than holding one HTTP request for as long as the original box was driven.

Actions and commands are done again. A file write and a clipboard set are not: the trace records that they happened and not the bytes they carried, so they come back in `skipped` with the reason. A fork short of its original in a way nothing reports would be worse than one that says where it is short.

`"mode": "snapshot"` is refused. Copying a running machine needs a substrate that can freeze one, and a container runtime cannot checkpoint an X session.

## Boxes that should not still be here

A deadline is armed by the engine when a box is launched — by a task in the process that launched it. Take a box back after a restart and nobody is counting for it any more, so the server sweeps the label the engine wrote, which is what that label is for. It also lets go of anything the runtime no longer holds, so `GET /v1/boxes` does not list a box that answers nothing.

Both show up in the trace as `gone`, with what happened:

```
system  gone  its deadline passed
system  gone  the runtime no longer has it
```

`COMPUTER_SERVER_REAP_SECS` sets the cadence, 30s by default.

A box that names no `expires_after_secs` takes its runtime's `lifetime`, one hour by default, so nothing runs until somebody notices it. `expires_after_secs` and `idle_timeout_secs` under 60s are refused. The clock starts when a box is created rather than when it is ready, so a shorter deadline removes it mid-launch and the caller waits out the full ready timeout to be told the container went missing.

## Surviving a restart

A box is a container and this server is a process, and the container is the one that survives. On startup it takes back every box it finds still running, so a restart does not leave them charging for memory nobody is using:

```
INFO took a box back box_=box_9cf78792… runtime=docker
INFO took back boxes left running by an earlier server taken=1
```

Each box record names the runtime it was created on, so a restart goes there first. A box whose runtime is no longer configured is listed as `unreachable` with the reason, and its record is kept: it comes back when the runtime does.

Then every runtime is scanned for the `computer.server.box` label, which carries the box's own spec where the runtime keeps it rather than where this process does. So a box the store lost still comes back as one this server can drive *and* fork, rather than a name it has to guess about.

With the default memory store, **the trace does not come back.** An adopted box starts a new trace that says `adopted`, and a fork can replay only what happened since. A durable storage backend keeps earlier entries and frames across the restart.

## What happened to a box

```bash
curl -s "localhost:8080/v1/boxes/$BOX/trace?after=12&limit=100"
curl -s localhost:8080/v1/boxes/$BOX/trace/frames/$HASH -o frame.png
```

Every action is recorded with the actor that asked for it, and the action is carried whole, so a run can be replayed against a fresh box. A trace outlives the box it describes — removing a box must not remove the record of what was done in it.

```
  0 agent   box_created  1280x800
  1 agent   acted  open_url ok=True
  2 agent   frame frame=20376bc5
  3 agent   takeover_started
  4 agent   acted  open_url ok=True
  5 person  frame frame=4fe5823c
  6 agent   takeover_ended
  7 agent   box_deleted
```

**A person's keystrokes are not in here, and cannot be.** They arrive over VNC and go straight into the box, so nothing out here sees one. What is recorded is custody: the interval a screen was theirs. On a frame the claim is weaker still — the actor is whoever *held* the screen when it was captured, not whoever changed it, and an agent can still act during a handover.

Frame entries are written only when the screen actually moved, so polling a still screen adds nothing — and no frame entry between a takeover's two ends means nothing visible happened while it was held.

## Current limits

Snapshot fork is refused because nothing here can freeze a running desktop.

The default memory store is bounded to 10,000 entries and 256 distinct frames per box, across 256 boxes, and a restart forgets it. Durable backends use the configured retention windows instead. The boxes themselves come back; see above.
