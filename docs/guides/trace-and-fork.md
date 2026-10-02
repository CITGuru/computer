# Trace and fork

`holmd` records what is done to each box in a trace. You can read the trace to see what happened, and fork a box to make a new box that does the same steps again.

Traces and forks need a server. The Rust library with no server has no trace. See [The server](../concepts/server.md).

## What the trace records

Each entry has a sequence number (`seq`), a time (`at_ms`), an actor, an event, and sometimes a frame hash.

| Actor | Meaning |
| --- | --- |
| `agent` | A client of the API: the CLI, an MCP tool, or a program. |
| `person` | A person who has the screen during a takeover. |
| `system` | The server, for example when it removes an expired box. |

| Event (`kind`) | Recorded when |
| --- | --- |
| `box_created` | The box is created. Contains the full spec and placement. |
| `forked_from` | The box is a fork. Names the source box and `up_to`. |
| `adopted` | A server took the box back after a restart. |
| `acted` | An action ran, with the full action, `ok`, and the error if it failed. |
| `frame` | The screen changed. The entry has the frame hash. |
| `executed` | A command ran, with its arguments, exit code, and `timed_out`. |
| `app_launched` | An application was opened, with its window. |
| `apps_installed` | Applications were installed into the running box. |
| `file_written`, `file_read` | A file was written or read, with the path and size. Not the content. |
| `clipboard_set`, `clipboard_read` | The clipboard was set or read. Not the text. |
| `page_captured` | A page screenshot or PDF was made. |
| `takeover_started`, `takeover_ended` | A person got the screen, and gave it back. |
| `box_paused`, `box_resumed`, `box_stopped`, `box_started` | Lifecycle changes. |
| `gone` | The box was removed by the server, with the reason. |
| `box_deleted` | A client removed the box. |

### What is not recorded

- **A person's input.** It goes to the box over VNC, so the server does not see it. The trace records only the period when the person had the screen. The frames in that period show what changed.
- **Page actions outside a batch.** `holm browser` commands and the MCP page tools (`click_element`, `fill_field`, `dropdown`, and others) are not in the trace. Page actions inside an action batch are. See [Make a run that you can fork](#make-a-run-that-you-can-fork).
- **Reads.** `read`, `snapshot`, `find`, and `evaluate` through their own endpoints are not recorded.

The actor of a `frame` entry is the one that had the screen when the frame was captured, not the one that changed it. A frame is recorded only when the screen changed, so a still screen adds no entries. No `frame` entry between `takeover_started` and `takeover_ended` means that nothing visible changed during the takeover.

## Read the trace

```bash
holm trace "$BOX"
holm trace "$BOX" --after 40
```

```text
   0  agent    created 1280x800
   1  agent    screen 0  open_url
   2  agent    screen 0  the screen changed
   3  agent    screen 0 handed to a person
   4  person   screen 0  the screen changed
   5  agent    screen 0 taken back
   6  agent    ran ls /tmp → 0
```

REST:

```bash
curl -s "$BASE/v1/boxes/$BOX/trace?after=12&limit=100"
curl -s "$BASE/v1/boxes/$BOX/trace/frames/$HASH" -o frame.png
```

The response has `entries` and `next`. To read more, send `next` as `after`. A page has at most 500 entries.

A trace stays after its box is removed.

## Make a run that you can fork

A fork does again only some of what the trace has:

| Interface | Replayed by a fork | Recorded, but not replayed |
| --- | --- | --- |
| CLI | `open`, `mouse`, `keyboard`, `wait`, `app`, `exec`, `batch` | `clip` (set), `file put` |
| MCP | `open_url`, `click`, `type_text`, `press_key`, `drag`, `scroll`, `wait_until_still`, `open_app`, `run_command`, `batch` | `clipboard` (set), `write_file` |
| REST | Each action in `POST …/actions`, and `POST …/exec` | `PUT …/clipboard`, `PUT …/files`, `POST …/apps` |

To make page actions part of a run that you can fork, put them in a batch:

```bash
cat > steps.json <<'JSON'
[
  { "type": "open_url", "url": "https://example.com/order" },
  { "type": "on_page", "what": { "op": "fill", "query": "Name", "text": "Ada" } },
  { "type": "on_page", "what": { "op": "click", "query": "Continue" } },
  { "type": "on_page", "what": { "op": "wait_for", "query": "Details" } }
]
JSON
holm batch "$BOX" steps.json
```

MCP: the `batch` tool, with steps such as `fill_field` and `click_element`.

Each step of a batch becomes its own `acted` entry.

## Fork a box

```bash
NEW=$(holm fork "$BOX")
NEW=$(holm fork "$BOX" --up-to 40)
```

REST:

```bash
curl -s -X POST "$BASE/v1/boxes/$BOX/fork" \
  -H 'content-type: application/json' \
  -H 'idempotency-key: fork-1' \
  -d '{"up_to": 40}'
```

| Field | Effect |
| --- | --- |
| `up_to` | Do the steps up to this sequence number. Default: all. |
| `placement` | A placement for the new box. It replaces the source placement completely. |
| `mode` | `replay` (default). `snapshot` is refused. |

MCP: `fork_box`. It forks the full trace. It has no `up_to` or `placement`.

The server:

1. Reads the spec and placement from the `box_created` entry.
2. Creates a new box on the same runtime, unless your `placement` names another. The new box does not get the source's browser profile.
3. Records `box_created` and `forked_from` in the new trace.
4. Does the recorded steps again, in order, at about the original speed. A wait between two steps is at most 2 seconds.
5. Captures a frame at the end.

You can fork a box that was removed, because its trace stays.

### The result

```json
{
  "box": { "id": "box_…", "state": "ready" },
  "replay": {
    "attempted": 3,
    "ok": 3,
    "truncated": false,
    "skipped": [
      { "seq": 2, "kind": "file_written", "why": "the trace records that /tmp/marker.txt was written, not what went into it" }
    ]
  }
}
```

| Field | Meaning |
| --- | --- |
| `attempted`, `ok` | How many steps ran, and how many succeeded. |
| `stopped_at` | The source `seq` of the step that failed. The fork stops at the first failure. |
| `truncated` | `true` if the fork stopped after 3 minutes. |
| `skipped` | Steps that the fork could not do again, with the reason. |

The CLI prints the new box ID. The new box stays, even when the replay stopped early.

## Why a fork can be different

A fork does the steps again. It does not copy memory or disk. The result can differ from the source because:

- The page can be different now.
- The network and page timing can be different.
- A dialog or a popup can appear this time and not the first time.
- Animations make two screens of the same state look different.

Compare the last frame of each trace if you must know how close the fork is.

What the fork does not do again:

| Entry | Why |
| --- | --- |
| `acted` with `ok: false` | The source refused it, so it did not happen. |
| `file_written` | The trace does not keep the content. Listed in `skipped`. |
| `clipboard_set` | The trace does not keep the text. Listed in `skipped`. |
| `apps_installed` | Not done again, and not listed in `skipped`. Put applications in the spec so that the fork's image has them. |
| A person's input | Not in the trace. |

## Keep traces across restarts

The default memory store loses all traces when the server stops. It also keeps at most 10,000 entries and 256 frames for each box, and traces for at most 256 boxes.

After a restart with the memory store, the server takes its boxes back, but each trace starts again with `adopted`. A fork of such a box does only the steps after the restart.

Use a durable store to keep traces:

```bash
HOLM_STORAGE_BACKEND=local HOLM_STATE_DIR=/var/lib/holm holmd
```

| Variable | Default | Effect |
| --- | --- | --- |
| `HOLM_KEEP_FRAMES_SECS` | 2 hours | How long to keep frames. |
| `HOLM_KEEP_ENTRIES_SECS` | 7 days | How long to keep trace entries. |
| `HOLM_PRUNE_SECS` | 1 hour | How often to remove old data. |

A fork needs the frames only for comparison, not to replay, so old frames can go before the entries. See [Storage](../concepts/server.md#storage).
