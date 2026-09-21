# computer-mcp

An MCP server that gives an agent a desktop.

The same tools are available through stdio from `computer mcp --stdio` and through Streamable HTTP from `computerd` at `/mcp`.

## Stdio configuration

Install the root package, which provides `computer` and `computerd`:

```bash
cargo install --path . --locked
computerd
```

Configure a host that launches local MCP processes:

```json
{
  "mcpServers": {
    "computer": {
      "command": "computer",
      "args": ["mcp", "--stdio"],
      "env": {
        "COMPUTER_SERVER_URL": "http://127.0.0.1:8080"
      }
    }
  }
}
```

Add `COMPUTER_SERVER_TOKEN` when the selected server requires one. If no server is configured or listening, `computer mcp` starts an embedded server. Use a long-lived `computerd` when traces, forks, expiry, or durable storage matter.

Set `COMPUTER_CONTENT_BOUNDARIES=1` to have `read_page`, `snapshot`, `find`, `evaluate` and `console` put what the page wrote between two markers holding a nonce the page cannot know, so a model can tell the page's words from the tool's. On `computerd` it covers `/mcp`.

Logs go to stderr. Stdout carries JSON-RPC only.

## Streamable HTTP configuration

`computerd` serves MCP beside REST:

```text
URL:           https://boxes.example.com/mcp
Transport:     Streamable HTTP
Authorization: Bearer <COMPUTER_SERVER_TOKEN>
```

The same bearer protects `/v1` and `/mcp`. A loopback server can run without one. A non-loopback bind is refused unless `COMPUTER_SERVER_TOKEN` is set.

Example:

```bash
COMPUTER_SERVER_ADDR=0.0.0.0:8080 \
COMPUTER_SERVER_TOKEN="$(openssl rand -hex 32)" \
COMPUTER_PUBLIC_URL=https://boxes.example.com \
computerd
```

Terminate TLS in a reverse proxy and forward HTTP and WebSocket upgrades. `COMPUTER_PUBLIC_URL` is the public origin used by the live screen app when the proxy hides the request origin.

## MCP Apps live screen

Hosts that implement MCP Apps can render `ui://computer/screen.html` beside results from `launch_box`, `open_screen`, and `hand_over`. The app shows the live desktop and lets a person watch, take control, return control, and record.

Each tool result gives the app a short-lived ticket for one screen under `_meta`. The model does not receive that ticket. The browser connects back to `computerd`, so the box ports can stay private.

## The tools

Box and screen management:

`launch_box` · `list_boxes` · `inspect_box` · `remove_box` · `pause_box` · `stop_box` · `resume_box` · `open_screen` · `screen_status` · `fork_box`

Pages:

`open_url` · `tabs` · `read_page` · `page_screenshot` · `page_pdf` · `snapshot` · `find` · `click_element` · `fill_field` · `focus` · `check` · `dropdown` · `upload_file` · `wait_for` · `hover` · `highlight` · `drag_element` · `history` · `scroll_page` · `evaluate` · `console` · `dialog`

Desktop and native applications:

`screenshot` · `click` · `mouse_down` · `mouse_up` · `type_text` · `press_key` · `key_down` · `key_up` · `scroll` · `wait_until_still` · `drag` · `cursor` · `window` · `widget` · `open_app` · `list_apps` · `record`

Files and commands:

`list_files` · `grep` · `glob` · `read_file` · `write_file` · `clipboard` · `run_command`

Browser state:

`save_state` · `load_state` · `cookies`

Coordination:

`batch` · `hand_over` · `reclaim_screen`

## Typical task

1. Call `launch_box` and keep the returned `box_id`.
2. Call `open_url`.
3. Call `snapshot` to inspect and number the page controls.
4. Use `fill_field`, `dropdown`, `check`, and `click_element` with names or `@eN` references.
5. Use `wait_for` after an action starts a navigation or fetch.
6. Call `snapshot` with `delta` to inspect only later changes.
7. Call `remove_box` when the task is complete.

Use `screenshot` plus `click`, `type_text`, `press_key`, and `drag` for desktop applications. Launch the box with accessibility enabled and use `widget` for native forms and dialogs.

**On a web page, act by name rather than by coordinate.** `snapshot` lists every control in order and numbers each, so `@e12` is then a query, and with `delta` it answers only what appeared, changed or left since the last one; every action's answer ends the same way, so a menu that opened is named by ref without another snapshot. `find` says what is there; `click_element`, `fill_field`, `dropdown` and `upload_file` act on what a query names. A point taken from a screenshot is wrong the moment the page moves under it, and two of these have no coordinate at all: a file chooser is the operating system's window, and a native dropdown opens a menu no screenshot shows and no click reaches. `click`, `type_text` and `drag` remain for everything that is not a page.

Field, focus, dropdown, and upload operations follow an explicit HTML label to its control. They do not guess from arbitrary nearby text. A refusal names nearby fields by reference or selector when it can, and asks for a snapshot when the match is ambiguous.

**In a native window, act by name too.** `widget` reads the accessibility tree — the roles and names a toolkit publishes about its own widgets — so a file dialog or a settings panel is reachable the same way a page is. It needs a box built with `Feature::Accessibility`, and it matches the label beside a field as well as the field's own name, because a form field usually has no name of its own. `press` there runs the widget's own action and sends no pointer event, so it reaches something covered or scrolled out of view; where an application is watching the pointer, `find` answers with a rectangle and `click` still works.

`wait_for` **after anything that makes the page fetch.** A sleep is either short enough to act too early or long enough to be paid on every step, and a page that answers a click by fetching says nothing when it starts and everything when the result arrives. `gone` waits the other way, for a spinner ending or a dialog closing.

**Two scrolls, and they are not the same.** `scroll` sends wheel clicks at a screen point, so it needs a coordinate and moves whatever sits under the pointer — right for a desktop application. `scroll_page` moves the page itself in pixels and answers with where it stopped, which is how a caller tells that a page loading more on arrival has run out: the same position twice.

`read_page` and `screenshot` answer different questions. A frame says **where** to click, which is the only place a coordinate can come from. `read_page` says **what is there** — as text rather than a picture of text, past the fold, and with the address behind each link instead of its label.

**Every tool that moves the screen answers with the frame it produced**, as an image rather than a hash. An agent that has to ask for a screenshot after every click spends two round trips on one step, and the second one is where it forgets to look.

A tool that fails answers with `isError` and the reason, not a protocol error — the model is the one that has to act on it, and a JSON-RPC error never reaches it. Only a malformed call gets a protocol error.

`hand_over` returns a URL a person opens, and holds the agent's input back until `reclaim_screen`. `fork_box` builds a second box by doing again what was done to the first; it reconstructs rather than copies, so the two will be close and rarely identical.
