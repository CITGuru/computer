# computer-mcp

An MCP server that gives an agent a desktop.

Speaks JSON-RPC over stdio to whatever launched it, and HTTP to a
`computer-server` — so the boxes can be on this machine or on a fleet somewhere
else, and nothing about the tools changes.

```bash
cargo run -p computer-server            # the boxes live here
cargo install --path crates/computer-mcp
```

```json
{
  "mcpServers": {
    "computer": {
      "command": "computer-mcp",
      "env": {
        "COMPUTER_SERVER_URL": "http://127.0.0.1:8080",
        "COMPUTER_SERVER_TOKEN": "…"
      }
    }
  }
}
```

## The tools

`launch_box` · `list_boxes` · `remove_box` · `screenshot` · `read_page` ·
`find` · `wait_for` · `click_element` · `fill_field` · `dropdown` ·
`upload_file` · `hover` · `history` · `scroll_page` ·
`open_url` · `open_app` · `list_apps` · `click` · `type_text` · `press_key` ·
`scroll` · `drag` · `run_command` · `hand_over` · `reclaim_screen` · `fork_box`

**On a web page, act by name rather than by coordinate.** `find` says what is
there; `click_element`, `fill_field`, `dropdown` and `upload_file` act on what
a query names. A point taken from a screenshot is wrong the moment the page
moves under it, and two of these have no coordinate at all: a file chooser is
the operating system's window, and a native dropdown opens a menu no screenshot
shows and no click reaches. `click`, `type_text` and `drag` remain for
everything that is not a page.

**`wait_for` after anything that makes the page fetch.** A sleep is either
short enough to act too early or long enough to be paid on every step, and a
page that answers a click by fetching says nothing when it starts and
everything when the result arrives. `gone` waits the other way, for a spinner
ending or a dialog closing.

**Two scrolls, and they are not the same.** `scroll` sends wheel clicks at a
screen point, so it needs a coordinate and moves whatever sits under the
pointer — right for a desktop application. `scroll_page` moves the page itself
in pixels and answers with where it stopped, which is how a caller tells that a
page loading more on arrival has run out: the same position twice.

`read_page` and `screenshot` answer different questions. A frame says **where**
to click, which is the only place a coordinate can come from. `read_page` says
**what is there** — as text rather than a picture of text, past the fold, and
with the address behind each link instead of its label.

**Every tool that moves the screen answers with the frame it produced**, as an
image rather than a hash. An agent that has to ask for a screenshot after every
click spends two round trips on one step, and the second one is where it forgets
to look.

A tool that fails answers with `isError` and the reason, not a protocol error —
the model is the one that has to act on it, and a JSON-RPC error never reaches
it. Only a malformed call gets a protocol error.

`hand_over` returns a URL a person opens, and holds the agent's input back until
`reclaim_screen`. `fork_box` builds a second box by doing again what was done to
the first; it reconstructs rather than copies, so the two will be close and
rarely identical.

## Stdout carries the protocol

Logs go to stderr. A stray line on stdout is a parse error at the other end.
