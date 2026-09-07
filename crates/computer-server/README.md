# computer-server

A REST API over `computer` boxes. Create them, drive them, take them away —
all of it over HTTP, so a shell script with `curl` and an MCP server built by
mapping tools onto endpoints both work with no SDK in between.

```bash
cargo run -p computer-server            # 127.0.0.1:8080, or $COMPUTER_SERVER_ADDR
```

## The gate

Whoever reaches this API creates boxes, drives them, reads their frames and runs
commands inside them, so an address it answers on is worth more than any single
viewer URL behind it.

The rule is the engine's own. On loopback it opens without a token, which is
what a box on a laptop has always been. Bound anywhere else it needs one, and
says so at startup rather than on the first unauthenticated request:

```
$ COMPUTER_SERVER_ADDR=0.0.0.0:8080 cargo run -p computer-server
Error: 0.0.0.0:8080 can be reached from off this host and COMPUTER_SERVER_TOKEN
is not set. Whoever reaches this API can create boxes, drive them and run
commands inside them. Set a token, or bind to 127.0.0.1.
```

```bash
export COMPUTER_SERVER_TOKEN=$(openssl rand -hex 32)
curl -s localhost:8080/v1/boxes -H "Authorization: Bearer $COMPUTER_SERVER_TOKEN"
```

The token is a `computer::Secret`, so it is refused under 16 characters, and it
has no `Display` and no `Serialize` — it cannot reach a log by accident.
Comparison is constant-time. `/v1/health` answers without one, since a load
balancer has no token and a refusal would tell whoever asked the same thing.

This gates the API. The viewer and control URLs it hands back carry their own
credentials, set by `policy.auth` in the spec.

## Create a box

A box is described by a **spec** — what desktop is wanted — and placed by a
**placement** — where it runs and for how long. They are separate because two
identical desktops that differ only in a memory limit are one desktop, and
`spec_digest` is what says so. Both live in `computer-types`, which describes a
desktop and how to address one without knowing an API exists.

`width`, `height` and `screens` are optional. A spec that leaves them open
takes whatever the image gives, and is a different spec from one that pins
them.

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

Unknown keys in a spec are refused rather than ignored: a misspelled key that
is quietly dropped hands back a box missing the thing it was misspelled for.

## Drive it

An agent's step is several actions and one look, so that is the request. The
frame the actions produced comes back in the same response.

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

- **A batch stops at its first failure.** A click that follows a move which
  failed lands wherever the pointer was, and the frame afterwards looks like
  it worked. `stopped_at` names the action that ended the run, and everything
  after it was not attempted.
- **`have_frame` costs nothing when nothing moved.** Frames are named by their
  contents. A screen that has not changed answers `"unchanged": true` and
  carries no picture.
- **Send an `idempotency-key` on anything that acts.** A retried click is a
  double click, which on a real interface opens the file rather than selecting
  it. A key is bound to the request it first arrived on, so use a fresh one per
  operation: the same key on a different body or a different endpoint answers
  `409` rather than handing back the first request's reply for work that never
  happened.

## Open an app

An app is named, not commanded: a caller who could post an argv to a driving
endpoint would be running programs through one meant for clicks, and `exec`
already does that in the open.

```bash
curl -s localhost:8080/v1/boxes/$BOX/screens/0/actions \
  -H 'content-type: application/json' -d '{
    "actions": [{ "type": "launch", "app": "gimp" }],
    "want": ["frame"]
  }'
```

**The call returns once the app has drawn**, and answers with the window it
drew. That is the whole point of it. A window exists well before the program
behind it has painted — GIMP maps a splash screen carrying its own class about
half a second before the real one, and VS Code maps its window and paints a
second later — so a launch that returned on the window appearing would hand
back a screen the next click lands wrong on.

The app has to be in the box already: `GET /v1/catalog` names what this server
knows, and `spec.apps` is what installs one when the box is created.

## Read the page

A frame says where to click. It does not say what a page holds: it is a picture
of text, shows one viewport of a document that may be far longer, and renders a
link as its label rather than its address.

```bash
curl -s "localhost:8080/v1/boxes/$BOX/page?format=markdown&limit=4000&max_links=20"
```

`limit` and `max_links` are optional and cap what comes back — ask for a few
hundred characters to decide whether a page is worth reading, and `truncated`
says whether that cut anything. Both are ceilings here rather than defaults a
caller can raise; `computer::Page::read` has no ceiling, because a library owes
its caller none and a deployment owes one to whoever it answers.

`format` is `markdown` (the default), `text` or `raw`. Markdown keeps the
headings, lists, tables and code a flat rendering loses, and puts each link's
address beside its words. `text` is the cheapest answer to "what does this
say". `raw` is the document's own HTML — an escape hatch for what the other two
do not carry, and megabytes where they are kilobytes.

A page marking `<main>` or `<article>` is read from there, since HTML already
defines those as its content. Where neither exists the whole body is read:
telling chrome from content without them is a real heuristic, and a wrong one
loses the page. Wikipedia marks a `<main>` that holds its own sidebar, so a
chrome-heavy page can still spend a reader's budget before its article
begins — raise `limit` or go straight to the section you want.

Answers with the visible page's title, URL, rendered text and its links with
their `href`s — so a caller reads what is there, and navigates by URL rather
than guessing a coordinate for an anchor. Clicking is still how everything that
is not a link is reached.

The read is scoped rather than an `evaluate` endpoint: running a caller's
JavaScript in the box's browser is a wider door than any tool here needs.

The reading itself is `computer::Page::read`, so a library user gets it without
a server and this endpoint is only the HTTP in front of it.

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
| `GET /v1/catalog` | the app names a launch can ask for |
| `GET /v1/boxes/{id}/page?limit=` | the page on screen, as text and links |
| `GET /v1/boxes/{id}/screens/{n}/windows` | what is on the screen |
| `POST …/windows/{w}/focus`, `DELETE …/windows/{w}` | raise one, close one |
| `POST /v1/boxes/{id}/exec` | one command, one answer |
| `GET`/`PUT` `/v1/boxes/{id}/files` | base64 in, base64 out |

Every error is the same shape — `code`, `message`, `retryable` — including a
body that is not JSON at all, because that is the first error most clients
ever see.

The wire types live in [`computer-api`](../computer-api), and
[`computer-client`](../computer-client) is a Rust client over them.

## Fork a box

```bash
curl -s -X POST localhost:8080/v1/boxes/$BOX/fork \
  -H 'content-type: application/json' -H 'idempotency-key: fork-1' \
  -d '{ "up_to": 42 }'
```

Launches a box from the same spec and does again what was done to the first
one, at roughly the original pace. It reads the *trace*, not the box, so **a box
that has been removed can still be forked** — its record outlived it and carries
the spec.

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

**A replay reconstructs, it does not copy.** The same actions against a page
that has since changed, a slower network, or a dialog that appeared this time
land somewhere else. Even a faithful replay rarely produces identical pixels —
a desktop animates, and a tooltip caught mid-fade is a different picture of the
same state. Compare frames yourself if you need to know how close it got; both
boxes' traces end with one.

Actions the original was refused are skipped, since replaying them would invent
a difference. A replay stops at the first failure and names the source sequence
that stopped it, and gives up after three minutes rather than holding one HTTP
request for as long as the original box was driven.

Actions and commands are done again. A file write and a clipboard set are not:
the trace records that they happened and not the bytes they carried, so they
come back in `skipped` with the reason. A fork short of its original in a way
nothing reports would be worse than one that says where it is short.

`"mode": "snapshot"` is refused. Copying a running machine needs a substrate
that can freeze one, and a container runtime cannot checkpoint an X session.

## Boxes that should not still be here

A deadline is armed by the engine when a box is launched — by a task in the
process that launched it. Take a box back after a restart and nobody is counting
for it any more, so the server sweeps the label the engine wrote, which is what
that label is for. It also lets go of anything the runtime no longer holds, so
`GET /v1/boxes` does not list a box that answers nothing.

Both show up in the trace as `gone`, with what happened:

```
system  gone  its deadline passed
system  gone  the runtime no longer has it
```

`COMPUTER_SERVER_REAP_SECS` sets the cadence, 30s by default.

`expires_after_secs` and `idle_timeout_secs` under 60s are refused. The clock
starts when a box is created rather than when it is ready, so a shorter deadline
removes it mid-launch and the caller waits out the full ready timeout to be told
the container went missing.

## Surviving a restart

A box is a container and this server is a process, and the container is the one
that survives. On startup it takes back every box it finds still running, so a
restart does not leave them charging for memory nobody is using:

```
INFO took a box back box_=box_9cf78792… runtime=docker
INFO took back boxes left running by an earlier server taken=1
```

Each box carries its own spec in a `computer.server.box` label, written where
the runtime keeps it rather than where this process does. So what comes back is
a box this server can drive *and* fork, rather than a name it has to guess
about.

Set `COMPUTER_SERVER_RUNTIMES=docker,podman` to look in more than one. A box
placed on a runtime nobody asks about stays lost.

**The trace does not come back.** It lived in memory and the box did not, so an
adopted box starts a new one that says `adopted` and nothing before it. Forking
one replays only what has happened since.

## What happened to a box

```bash
curl -s "localhost:8080/v1/boxes/$BOX/trace?after=12&limit=100"
curl -s localhost:8080/v1/boxes/$BOX/trace/frames/$HASH -o frame.png
```

Every action is recorded with the actor that asked for it, and the action is
carried whole, so a run can be replayed against a fresh box. A trace outlives
the box it describes — removing a box must not remove the record of what was
done in it.

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

**A person's keystrokes are not in here, and cannot be.** They arrive over VNC
and go straight into the box, so nothing out here sees one. What is recorded is
custody: the interval a screen was theirs. On a frame the claim is weaker
still — the actor is whoever *held* the screen when it was captured, not
whoever changed it, and an agent can still act during a handover.

Frame entries are written only when the screen actually moved, so polling a
still screen adds nothing — and no frame entry between a takeover's two ends
means nothing visible happened while it was held.

## Not here yet

`spec.apps` parses and is **refused**, because there is no catalog behind it
and a box handed back without the apps it named would look like the one that
was asked for. Snapshot fork is refused, because nothing here can freeze a
running desktop.

Traces are held in memory and bounded — 10,000 entries and 256 distinct frames
per box, 256 boxes — so this is a record rather than an archive, and a restart
forgets it. The boxes themselves come back; see above.
