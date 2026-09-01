# computer-server

A REST API over `computer` boxes. Create them, drive them, take them away —
all of it over HTTP, so a shell script with `curl` and an MCP server built by
mapping tools onto endpoints both work with no SDK in between.

```bash
cargo run -p computer-server            # 127.0.0.1:8080, or $COMPUTER_SERVER_ADDR
```

## Create a box

A box is described by a **spec** — what desktop is wanted — and placed by a
**placement** — where it runs and for how long. They are separate because two
identical desktops that differ only in a memory limit are one desktop, and
`spec_digest` is what says so. Both live in `computer-spec`, which describes a
desktop without knowing an API exists.

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
  it.

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
| `POST /v1/boxes/{id}/exec` | one command, one answer |
| `GET`/`PUT` `/v1/boxes/{id}/files` | base64 in, base64 out |

Every error is the same shape — `code`, `message`, `retryable` — including a
body that is not JSON at all, because that is the first error most clients
ever see.

## Not here yet

`spec.apps` parses and is **refused**, because there is no catalog behind it
and a box handed back without the apps it named would look like the one that
was asked for. Snapshots, fork and traces are not built. Boxes live in this
process's memory, so a restart forgets them while their containers keep
running — `computer sweep` is what collects those today.
