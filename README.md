# Computer

`computer` provides isolated desktop boxes that agents can use to complete and drive real tasks. Each computer runs a Linux desktop inside a container or microVM, where an agent can open web pages, drive native apps, take screenshots, move the pointer, type text, run commands, and transfer files. You can also watch the agent work or take control through a browser when it needs help.

![Nine frames of a desktop being driven from Rust: a page opening, a URL typed, text selected by a drag, a context menu, a paste, and a second screen](./media/demo.gif)

## Contents

- [Quick start](#quick-start)
- [Choose an interface](#choose-an-interface)
- [Choose how to control the desktop](#choose-how-to-control-the-desktop)
- [Desktop operations](#desktop-operations)
- [Accessibility operations](#accessibility-operations)
- [Browser operations](#browser-operations)
- [Display and window operations](#display-and-window-operations)
- [Human control](#human-control)
- [Configure a desktop](#configure-a-desktop)
- [Box lifecycle and history](#box-lifecycle-and-history)
- [Server, REST, and MCP](#server-rest-and-mcp)
- [Runtimes](#runtimes)
- [Custom desktops and runtimes](#custom-desktops-and-runtimes)
- [What is inside the box](#what-is-inside-the-box)
- [Security](#security)
- [Examples](#examples)
- [Workspace crates](#workspace-crates)
- [Development and testing](#development-and-testing)

## Quick start

### Requirements

Install Rust 1.85 or newer and one supported runtime:

- Docker
- Podman
- nerdctl
- microsandbox, for a microVM instead of a container

There's no need to fetch or manage a separate desktop image. The image is built automatically from source the first time you launch a desktop. The initial build takes a few minutes, but later desktops with the same configuration start in seconds.

### Install the commands

Build and install `computer` and `computerd` from the current source:

```bash
git clone https://github.com/CITGuru/computer.git
cd computer
cargo install --path . --locked
```

`computer` controls desktops from a shell and serves MCP over stdio. `computerd` keeps the REST and HTTP MCP service running.

Start and control a desktop:

```bash
BOX=$(computer new)
computer open "$BOX" https://example.com
computer screenshot "$BOX" screen.png
computer rm "$BOX"
```

See the [CLI guide](crates/computer-cli/README.md) for server and remote-fleet use.

### Use the Rust API

Add the API without the command dependencies:

```toml
[dependencies]
computer = { git = "https://github.com/CITGuru/computer", default-features = false }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Launch a desktop, open a page, send input, and take a screenshot:

```rust
use computer::{Button, Computer, Point};

#[tokio::main]
async fn main() -> computer::Result<()> {
    let computer = Computer::launch().await?;

    computer.open_url("https://example.com").await?;
    computer.click(Point::new(640, 81), Button::Left).await?;
    computer.type_text("driven from rust").await?;

    let _png = computer.screenshot().await?;

    computer.shutdown().await
}
```

CLI:

```bash
BOX=$(computer new --url https://example.com)
computer mouse "$BOX" click 640 81 left
computer keyboard "$BOX" type "driven from the CLI"
computer screenshot "$BOX" screen.png
computer rm "$BOX"
```

`Computer::launch()` starts one 1280x800 screen. `viewer_url()` returns a browser URL where you can watch it. `shutdown()` stops and removes the container.

From this repository, run the complete example:

```bash
cargo run --example quickstart
```

## Choose an interface

### CLI

The `computer` command starts an ephemeral local server when no daemon is available. Start `computerd` when you need persistent traces, forks, remote access, or a fleet that survives command exits.

```bash
computerd

computer new --url https://example.com
computer ls
```

Set `COMPUTER_SERVER_URL` and, when required, `COMPUTER_SERVER_TOKEN` to use a remote server. Add `--local` to bypass the server for the smaller set of direct local commands.

See the [CLI guide](crates/computer-cli/README.md).

### Rust

The root `computer` package exports the desktop API. Disable its default `cli` feature when an application needs only the library:

```toml
computer = { git = "https://github.com/CITGuru/computer", default-features = false }
```

Optional features add the E2B client, microsandbox library binding, and daemon storage backends: `e2b`, `microsandbox`, `sqlite`, `postgres`, and `s3`.

### REST and Rust client

`computerd` exposes the complete remote API under `/v1`. The API supports box creation and lifecycle, action batches, frames, pages, windows, files, commands, takeovers, CDP access, traces, and forks.

Use [`computer-client`](crates/computer-client) from Rust or use HTTP directly. The wire types are in [`computer-api`](crates/computer-api).

### MCP

Use either interface:

- `computer mcp --stdio` for a local stdio MCP server
- `http://<server>/mcp` for Streamable HTTP served by `computerd`

Both interfaces expose the same boxes and tools. Hosts that support [MCP Apps](https://github.com/modelcontextprotocol/ext-apps) can show the live desktop beside tool results.

## Choose how to control the desktop

The crate provides three control methods. You can use more than one method in the same task.

**Screen coordinates** work with any visible application. Take a screenshot, select a point, and send mouse or keyboard input. Use this method when the application exposes no structured interface.

**Accessibility nodes** expose native widgets by role and name. Use them for file dialogs, settings panels, installers, and other native interfaces where coordinates can become stale.

**Chrome DevTools** controls Chromium pages directly. Use it to navigate, inspect the DOM, find elements, fill forms, evaluate JavaScript, and manage isolated browser sessions.

## Desktop operations

### Mouse and keyboard

Coordinates use device pixels. The top-left corner is `(0, 0)`.

```rust
let png = computer.screenshot().await?;

computer.move_to((640, 400)).await?;
computer.click((640, 400), Button::Left).await?;
computer.double_click((640, 400), Button::Left).await?;
computer.drag((100, 100), (400, 300), Button::Left).await?;

computer.type_text("hello").await?;
computer.press("ctrl+shift+p").await?;

computer.scroll((640, 400), Delta::down(3)).await?;
computer.scroll((640, 400), Delta::right(3)).await?;

let pointer = computer.cursor().await?;
```

CLI:

```bash
computer screenshot "$BOX" frame.png
computer mouse "$BOX" move 640 400
computer mouse "$BOX" click 640 400 left
computer mouse "$BOX" click 640 400 left --double
computer mouse "$BOX" drag 100 100 400 300 left
computer keyboard "$BOX" type "hello"
computer keyboard "$BOX" press "ctrl+shift+p"
computer mouse "$BOX" scroll 640 400 down 3
computer mouse "$BOX" scroll 640 400 right 3
computer mouse "$BOX" at
```

Complete Keyboard and Mouse reference:

```text
computer keyboard <box> type <text> [--delay MS]
computer keyboard <box> press <key>... [--held shift,ctrl,alt,super]

computer mouse <box> move <x> <y> [--smooth|--human] [--seed N]
computer mouse <box> click <x> <y> [left|right|middle] [--double]
                          [--held shift,ctrl,alt,super]
                          [--smooth|--human] [--seed N]
computer mouse <box> drag <x1> <y1> <x2> <y2> [left|right|middle]
                         [--held shift,ctrl,alt,super]
                         [--smooth|--human] [--seed N]
computer mouse <box> down [<x> <y>] [left|right|middle] [--hold SECONDS]
                         [--smooth|--human] [--seed N]
computer mouse <box> up [<x> <y>] [left|right|middle]

computer mouse <box> scroll [<x> <y>] up|down|left|right [NOTCHES]
computer mouse <box> scroll <x> <y> <DY> [DX]
computer mouse <box> at
```

The left button is the default. `--held` applies to a single click or a drag, not to `--double`. A scroll without coordinates uses the centre of the screen. In the signed form, positive `DY` moves down and positive `DX` moves right. `down` and `up` need the server and are not available with `--local`.

`scroll` moves the content below the given point. Positive `dy` moves down and positive `dx` moves right. A delta with both values sends one diagonal gesture.

Common key names work as expected. The crate converts names such as `enter`, `cmd`, and `pageup` to the display server's key names.

Keep these coordinate rules:

- Screenshots do not show the pointer. Use `cursor()` to read its position.
- Do not calculate a click from a scaled screenshot.
- Take a new screenshot after a person or another process changes the desktop.
- Take a new screenshot after an operation opens or raises a browser tab.

Hold modifiers as part of the pointer operation:

```rust
let screen = computer.primary();
let at = Point::new(640, 400);
let from = Point::new(100, 100);
let to = Point::new(400, 300);

screen.click_with(at, Button::Left, &[Held::Shift]).await?;
screen.drag_with(from, to, Button::Left, &[Held::Ctrl]).await?;
```

CLI:

```bash
computer mouse "$BOX" click 640 400 left --held shift
computer mouse "$BOX" drag 100 100 400 300 left --held ctrl
```

Modifier pointer operations are available on X11 and on Wayland.

Move the pointer with a smooth line or a repeatable human-like curve:

```bash
computer mouse "$BOX" move 640 400 --smooth
computer mouse "$BOX" click 640 400 left --human --seed 42
computer mouse "$BOX" drag 100 100 400 300 left --human --seed 42
```

The Rust API exposes the generated steps through `motion::path` and sends a full path in one operation:

```rust
use computer::{Desktop as _, Motion, Point, motion};

let from = Point::new(100, 100);
let to = Point::new(400, 300);
let steps = motion::path(from, to, Motion::Human, 42);
computer.move_along(&steps).await?;
computer
    .drag_along(from, &steps, Button::Left, &[Held::Shift])
    .await?;
```

Instant motion stays the default.

Use separate button operations only when a normal drag cannot express the interaction:

```bash
computer mouse "$BOX" down 400 400 left --hold 10
computer mouse "$BOX" move 900 600 --smooth
computer mouse "$BOX" up 900 600 left
```

The Rust API exposes the same low-level operations:

```rust
use computer::Desktop as _;

computer
    .button_down(Some(Point::new(400, 400)), Button::Left)
    .await?;
computer.move_to(Point::new(900, 600)).await?;
computer
    .button_up(Some(Point::new(900, 600)), Button::Left)
    .await?;
computer.let_go(Button::Left).await?;
```

The server holds a CLI `down` for 10 seconds by default and 60 seconds at most. It releases the button when that limit ends, when a person takes control, or at the end of an action batch.

Pace text for applications that drop characters:

```rust
computer
    .type_text("typed at a visible pace")
    .every(Duration::from_millis(20))
    .await?;

computer
    .press(["tab", "tab", "tab"])
    .holding([Held::Alt])
    .await?;
```

```bash
computer keyboard "$BOX" type "typed at a visible pace" --delay 20
computer keyboard "$BOX" press tab tab tab --held alt
```

`cursor()` reads the pointer without moving it. `find_cursor()` may move the pointer on a display server where probing is the only way to find it:

```rust
let last_known = computer.cursor().await?;
let measured = computer.find_cursor().await?;
```

### Wait for the screen

Wait for drawing to stop instead of using a fixed sleep:

```rust
computer
    .wait_until_still(
        Duration::from_millis(400),
        Duration::from_secs(10),
    )
    .await?;
```

CLI:

```bash
computer wait "$BOX" --settle 400 --within 10000
```

An animated screen can reach the deadline without becoming still.

### Screenshots and captures

`screenshot()` returns a full-size PNG. `capture()` can select a region or window and can reduce its size:

```rust
let screen = computer.primary();

let full = screen.screenshot().await?;
let region = screen
    .capture(&Shot::region(Rect::new(
        Point::new(100, 80),
        400,
        300,
    )))
    .await?;
let window_id = screen
    .windows()
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| computer::Error::denied("no window is open"))?
    .id;
let window = screen.capture(&Shot::window(&window_id)).await?;
let small = screen
    .capture(&Shot::window(&window_id).scaled(50))
    .await?;
```

CLI:

```bash
computer screenshot "$BOX" full.png
computer screenshot "$BOX" region.png --at 100,80 --size 400x300
computer screenshot "$BOX" window.png --window 42
computer screenshot "$BOX" small.png --window 42 --scale 50
computer screenshot "$BOX" pointer.png --pointer
```

A window capture looks up the current window position when it runs. Scaling reduces the bytes sent to an agent, but scaled captures must not be used to calculate input coordinates.

### Files and commands

Run commands and move files between the host and the desktop:

```rust
use computer::Search;

let output = computer.exec(["ls", "-la", "/tmp"]).await?;
let input = std::fs::read("input.png")?;
computer.write_file("/tmp/input.png", &input).await?;
let bytes = computer.read_file("/tmp/input.png").await?;
computer.upload("input.pdf", "/tmp/input.pdf").await?;
computer.download("/tmp/output.gif", "output.gif").await?;

let entries = computer.list_dir("/tmp").await?;
let matches = computer
    .grep(&Search {
        pattern: "needle".to_string(),
        path: "/workspace".to_string(),
        include: Some("*.rs".to_string()),
        ignore_case: false,
        limit: None,
    })
    .await?;
let paths = computer.glob("*.png", "/tmp", None).await?;

let logs = computer.logs().await?;
```

CLI:

```bash
computer exec "$BOX" -- ls -la /tmp
computer file "$BOX" put ./input.png /tmp/input.png
computer file "$BOX" get /tmp/output.gif ./output.gif
computer file "$BOX" ls /tmp
computer file "$BOX" grep "needle" /workspace --include "*.rs"
computer file "$BOX" glob "*.png" /tmp
```

Search results are capped in the box before they cross the wire. Commands in the direct Rust API have a two-minute default limit. Use `exec_within(argv, duration)` when a command needs a different limit.

The CLI `file` commands use the server API and are not available with `--local`.

### Clipboard

Each screen has its own clipboard and primary selection:

```rust
computer.set_clipboard("ready to paste").await?;
let text = computer.clipboard().await?;

computer
    .set_selection(Selection::Primary, "middle-click paste")
    .await?;
let selected = computer.selection(Selection::Primary).await?;
```

CLI:

```bash
computer clip "$BOX" "ready to paste"
computer clip "$BOX"
computer clip "$BOX" "middle-click paste" --primary
computer clip "$BOX" --primary
```

`CLIPBOARD` is used by copy and paste. `PRIMARY` is filled when text is selected and is pasted by a middle click.

Selections can also contain binary data:

```rust
let targets = computer
    .clipboard_targets(Selection::Clipboard)
    .await?;
let png = computer
    .clipboard_bytes(Selection::Clipboard, "image/png")
    .await?;
computer
    .set_clipboard_bytes(Selection::Clipboard, "image/png", &png)
    .await?;
```

While a person controls the screen, the program can read selections but cannot change them.

## Accessibility operations

Accessibility operations use the semantic tree that native applications publish through AT-SPI. Enable the required image packages when you launch the desktop:

```rust
let computer = Computer::builder()
    .accessibility()
    .launch()
    .await?;
let screen = computer.primary();
```

CLI:

```bash
BOX=$(computer new --accessibility)
```

### Read and find nodes

Read the tree or search it by widget name and role:

```rust
let tree = screen.nodes(None, Some(4)).await?;

let street = NodeQuery {
    query: "Street".to_string(),
    role: Some("text".to_string()),
    ..NodeQuery::default()
};

let fields = screen.find_nodes(&street, Some(10)).await?;
```

CLI:

```bash
computer widget "$BOX" tree --depth 4
computer widget "$BOX" find "Street" --role text --limit 10
```

`find_nodes` returns the best match first. A query can match a widget's own name or the label beside it. The `node.labelled` field shows when a nearby label produced the match.

### Focus, set, and invoke nodes

```rust
screen.focus_node(&street).await?;
screen.set_node(&street, "12 Bishop Street").await?;

let ok = NodeQuery {
    query: "OK".to_string(),
    ..NodeQuery::default()
};
screen.invoke_node(&ok, None).await?;
```

CLI:

```bash
computer widget "$BOX" focus "Street" --role text
computer widget "$BOX" fill "Street" "12 Bishop Street" --role text
computer widget "$BOX" press "OK"
```

`invoke_node` runs a widget action without moving the pointer. The toolkit controls the action names. GTK can call an action `click` while Qt calls it `Press`.

Each node can also contain its centre point. Use that point when the application must receive a real pointer click:

```rust
if let Some(at) = fields.first().and_then(|node| node.at) {
    screen.click(at, Button::Left).await?;
}
```

CLI:

```bash
computer mouse "$BOX" click 640 400 left
```

Applications and custom widgets can publish incomplete trees. Use screenshots and normal input when a useful node is not available.


## Browser operations

Browser operations use Chrome DevTools Protocol (CDP) to control Chromium by page structure instead of desktop pixels. They keep working when the Chromium window moves or another desktop window covers it.

Use three levels of browser control:

- `read` returns the rendered document as Markdown, text, or HTML.
- `snapshot`, `find`, and element actions inspect and operate on controls by query or reference.
- `eval` and `Page::call` are escape hatches for behavior that the higher-level API does not wrap.

Prefer page operations for websites. Use screenshots and desktop input for browser chrome, permission prompts, native file dialogs, and anything outside the page. Browser operations require a reachable CDP endpoint.

### Typical CLI workflow

Open a page, inspect its controls, act by reference or name, wait for the result, and read only what changed:

```bash
BOX=$(computer new --url https://www.selenium.dev/selenium/web/web-form.html)

computer browser "$BOX" snapshot --urls --quiet 400
# @e2 textbox "Text input"
# @e8 combobox "Dropdown (select)"
# @e12 checkbox "Default checkbox"
# @e15 button "Submit"

computer browser "$BOX" fill @e2 "agent"
computer browser "$BOX" select @e8 "Two"
computer browser "$BOX" check @e12
computer browser "$BOX" click @e15
computer browser "$BOX" wait "Received!" --within 10000
computer browser "$BOX" snapshot --delta
```

A query can be visible text, an accessible name, an element ID, a CSS selector, or an `@eN` reference from `snapshot`. Use a reference after you inspect a page because it identifies one specific element. Use text or a selector when no snapshot exists. Add `--exact` when a partial text match could select the wrong control.

Use the operation that matches the control:

- `fill` types into text fields and assigns values to controls such as dates, colours, and sliders.
- `check` and `uncheck` set checkbox state without toggling an already-correct value.
- `options`, `select`, and `deselect` operate on dropdowns.
- `upload` assigns files directly to a file input; it does not open a native chooser.
- `focus`, `hover`, `click`, and `drag` send the corresponding page interaction.

`fill`, `focus`, dropdown, and upload operations follow an explicit HTML label to its control, whether the label uses `for` or wraps the control. They do not guess that arbitrary nearby text names a field. If a query matches non-control content, the refusal names fields inside or beside it by reference or selector; when too many fields are nearby, it asks for a snapshot.

Do not use `fill` as a substitute for those specialized operations. It refuses a dropdown, checkbox, file input, or button and reports the correct operation.

### Open and manage pages

Chrome DevTools controls a page without screen coordinates:

```rust
let browser = computer
    .browser()
    .expect("a published DevTools port");

let mut page = browser
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;

let title = page.title().await?;
let links = page
    .evaluate("Array.from(document.links).map(a => a.href)")
    .await?;
let png = page.screenshot().await?;

page.navigate("https://example.org").await?;
```

CLI:

```bash
TAB=$(computer open "$BOX" https://example.com)
computer browser "$BOX" eval "document.title" --tab "$TAB"
computer browser "$BOX" eval "Array.from(document.links).map(a => a.href)" --tab "$TAB"
computer browser "$BOX" screenshot page.png --tab "$TAB"
computer open "$BOX" https://example.org --target current
```

`Page::call` sends any Chrome DevTools Protocol method that the crate does not wrap.

Use `open_page` rather than `open` followed by a load wait. A new tab first shows `about:blank`, which is already loaded.

### Find and act on elements

Pages provide higher-level operations for common browser tasks:

```rust
let fields = page.find("input", Some(10), None, None).await?;
page.fill("Email", "agent@example.com").await?;
page.choose(
    "Country",
    &["United Kingdom".to_string()],
    false,
)
    .await?;
page.upload("Attachment", &["/tmp/report.pdf".to_string()])
    .await?;
page.click_on("Submit", Button::Left).await?;
```

CLI:

```bash
computer browser "$BOX" find input --limit 10
computer browser "$BOX" fill "Email" "agent@example.com"
computer browser "$BOX" select "Country" "United Kingdom"
computer browser "$BOX" upload "Attachment" /tmp/report.pdf
computer browser "$BOX" click "Submit"
```

These operations use page structure. They do not depend on the position of the Chromium window.

The browser API also has operations for focus, checkboxes, radio buttons, dropdown options, hover, drag, history, scrolling, and waits. A query can be visible text, an accessible name, an ID, a selector, or an element reference from a snapshot.

### Read and snapshot a page

Read rendered page content as Markdown, text, or raw HTML:

```rust
let read = page
    .read(Reading::Markdown, Some(4_000), Some(20))
    .await?;

println!("{}\n{}", read.title, read.text);
```

```bash
computer browser "$BOX" read --limit 4000
```

A screenshot says where pixels are. A page read returns text beyond the viewport and the destination behind each link.

Snapshot the interactive controls in document order and address them by stable references:

```rust
let taken = page.snapshot(None, None).await?;
for element in &taken.elements {
    println!("{}", element.text);
}

page.click_on("@e4", Button::Left).await?;
let changed = page.snapshot_delta(None, None).await?;
```

```bash
computer browser "$BOX" snapshot --urls
computer browser "$BOX" click @e4
computer browser "$BOX" snapshot --delta
```

References stay with an element while the document stays loaded. A navigation clears them. A removed element leaves a hole instead of letting an old reference select a different control.

After a page has been snapshotted, server and CLI element actions also report the controls they made appear, change, or leave. This often removes the need for a second full snapshot.

### Wait for browser state

Wait for content, its removal, an enabled control, a quiet page, a loaded document, or a JavaScript condition:

```rust
page.wait_for("Done", false, Duration::from_secs(10))
    .await?;
page.quiet(
    Duration::from_millis(400),
    Duration::from_secs(10),
)
    .await?;
```

```bash
computer browser "$BOX" wait "Done" --within 10000
computer browser "$BOX" wait ".spinner" --gone
computer browser "$BOX" wait "Submit" --enabled
computer browser "$BOX" wait --load
computer browser "$BOX" wait --quiet 400
computer browser "$BOX" wait "Success" --or "Payment failed,Try again"
computer browser "$BOX" wait --fn "location.pathname === '/done'"
```

Use browser waits after an action starts a fetch. `--or` waits for the first of several outcomes. `--fn` waits until a JavaScript expression is truthy. Use screen stillness for desktop drawing that browser state cannot describe.

### Capture a page

A page screenshot excludes the desktop window, address bar, and pointer. It can capture the complete scrollable document:

```bash
computer browser "$BOX" screenshot page.png
computer browser "$BOX" screenshot page.jpg --full --format jpeg --quality 70
```

### Keep page and screen state aligned

`open_url()` opens and raises a new tab. Coordinates from an earlier screenshot then address the new frontmost page.

Use page visibility methods before you mix DevTools operations with screen coordinates:

```rust
let mut page = browser
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;

computer.open_url("https://example.net").await?;
assert!(!page.visible().await?);

page.bring_to_front().await?;
assert!(page.visible().await?);
```

CLI:

```bash
TAB=$(computer open "$BOX" https://example.com)
computer open "$BOX" https://example.net
computer browser "$BOX" tabs
computer browser "$BOX" switch "$TAB"
computer screenshot "$BOX" current-page.png --tab "$TAB"
```

`browser.visible_page().await?` returns the page that the screen currently shows.

### Connect another CDP library

Mint a short-lived CDP address through `computerd`:

```bash
export CDP=$(computer cdp "$BOX")
agent-browser --cdp "$(computer cdp "$BOX" --ws)" snapshot -i
```

Playwright can pass `$CDP` to `connectOverCDP`, and browser-use can use it as `cdp_url`. The address contains a token in its path. It lasts one hour by default, can be changed with `--ttl`, and expires with the box.

`computer cdp "$BOX" --direct` prints the unguarded loopback port instead. Use it only on the box host. The proxied address is the correct form for remote servers.

`computer cdp` uses the server API and is not available with `--local`.

### Isolate browser sessions

A browser group is a Chromium browser context. Groups share the Chromium process but keep cookies, local storage, IndexedDB, and service workers separate:

```rust
let group = browser.create_group().await?;
let mut page = group
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;

page.evaluate("localStorage.setItem('agent', 'one')")
    .await?;

group.close().await?;
```

Groups do not create screens. Only one page can be frontmost. Call `bring_to_front()` before you use screen coordinates.

### Carry a login between desktops

Export the parts of a browser session that belong to named origins:

```rust
let origins = ["https://example.com".to_string()];
let session = browser
    .export_session(&origins, Carry::default())
    .await?;

let tabs = browser.import_session(&session).await?;
```

`Carry::default()` includes cookies and local storage. Enable IndexedDB for sites that store login state there. Session data grants account access and must be handled as a credential.

### CLI command reference

These are all commands under `computer browser`:

```text
computer browser <box> read [--format markdown|text|raw] [--limit N] [--tab ID]
computer browser <box> snapshot [--scope QUERY] [--limit N] [--urls] [--delta]
                                [--quiet MS] [--tab ID]
computer browser <box> find [QUERY] [--role ROLE] [--exact] [--scroll]
                            [--limit N] [--tab ID]

computer browser <box> click <query> [--double] [--button left|right|middle]
                             [--smooth|--human] [--seed N] [--tab ID]
computer browser <box> drag <from-query> <to-query> [--button left|right|middle]
                            [--smooth|--human] [--seed N] [--tab ID]
computer browser <box> hover <query> [--smooth|--human] [--seed N] [--tab ID]
computer browser <box> focus <query> [--tab ID]
computer browser <box> fill <query> <value> [--tab ID]
computer browser <box> check <query> [--tab ID]
computer browser <box> uncheck <query> [--tab ID]

computer browser <box> options <query> [--tab ID]
computer browser <box> select <query> <option>... [--tab ID]
computer browser <box> deselect <query> [<option>...] [--tab ID]
computer browser <box> upload <query> <file>... [--in-box] [--tab ID]

computer browser <box> wait [QUERY] [--gone] [--or TEXT,TEXT] [--within MS]
                            [--quiet MS] [--enabled] [--load] [--fn JS]
                            [--exact] [--tab ID]
computer browser <box> back [--tab ID]
computer browser <box> forward [--tab ID]
computer browser <box> reload [--tab ID]

computer browser <box> eval <expression> [--timeout MS] [--limit N] [--tab ID]
computer browser <box> screenshot [FILE] [--full] [--format png|jpeg]
                                  [--quality N] [--tab ID]
computer browser <box> tabs
computer browser <box> switch <tab>
computer browser <box> close <tab>
```

Opening a page and exporting a CDP endpoint are top-level commands:

```text
computer open <box> <url> [--target blank|current]
computer cdp <box> [--ws] [--ttl MINUTES] [--direct]
```

## Display and window operations

### List and arrange windows

`window list` reports each window's ID, geometry, class, and title. Use the ID for later commands. Prefer the class when waiting for a window because a title often changes with the open document.

```rust
let screen = computer.primary();
let windows = screen.windows().await?;
let window = screen
    .active_window()
    .await?
    .or_else(|| windows.into_iter().next())
    .ok_or_else(|| computer::Error::denied("no window is open"))?;

screen
    .arrange(
        &window.id,
        Arrange::Size {
            width: 800,
            height: 600,
        },
    )
    .await?;
screen
    .arrange(&window.id, Arrange::At(Point::new(120, 90)))
    .await?;
```

The Rust API also exposes focus, window state, and close operations:

```rust
screen.focus(&window.id).await?;
screen.arrange(&window.id, Arrange::Maximise).await?;
screen.arrange(&window.id, Arrange::Minimise).await?;
screen.arrange(&window.id, Arrange::Restore).await?;
screen.close_window(&window.id).await?;
```

CLI:

```bash
computer window "$BOX" list
computer window "$BOX" active
computer window "$BOX" 42 size 800 600
computer window "$BOX" 42 move 120 90
computer window "$BOX" 42 focus
computer window "$BOX" 42 max
computer window "$BOX" 42 min
computer window "$BOX" 42 restore
computer window "$BOX" 42 close
```

Match a window by class when possible. A title can change with the open document.

Wait for a new window to appear and stop moving:

```rust
let dialog = screen
    .wait_for_window("Mousepad", Duration::from_secs(10))
    .await?;
```

CLI:

```bash
computer window "$BOX" wait Mousepad --within 10
```

The result reports the final geometry. A window manager can clamp or reject the requested size or position.

`window wait --within` uses seconds. `computer wait --within` and `browser wait --within` use milliseconds.

Complete CLI window reference:

```text
computer window <box> list
computer window <box> active
computer window <box> wait <class> [--within SECONDS]
computer window <box> <id> focus
computer window <box> <id> close
computer window <box> <id> move <x> <y>
computer window <box> <id> size <width> <height>
computer window <box> <id> max
computer window <box> <id> min
computer window <box> <id> restore
```

`focus` raises a window and then reports the window that actually has keyboard focus. `close` asks the application to close the window. Arrange commands return the final geometry because the window manager can clamp a move, enforce size hints, or refuse a state change.

### Use more than one screen

Screen IDs start at zero. Screen 0 starts with the desktop. Other screens start when they are first requested:

```rust
let second = computer.screen(ScreenId(1)).await?;
second.open_url("https://example.org").await?;
```

The bundled image supports up to eight screens. Each screen has separate browser data, clipboard selections, and view and control servers.

`screen()` leases the screen to the current process. A second caller is refused instead of receiving the same screen.

### Change the wallpaper

```rust
let bytes = std::fs::read("background.png")?;
computer.set_wallpaper(&bytes).await?;

let second = computer.screen(ScreenId(1)).await?;
second.set_wallpaper(&bytes).await?;
```

PNG and JPEG data are supported. A profile without wallpaper support returns `Unsupported`.

### Record the screen

Add video packages to record MP4 inside the box:

```rust
let computer = Computer::builder()
    .packages(Extras::video().packages)
    .launch()
    .await?;

computer
    .record(Duration::from_secs(10), "/tmp/screen.mp4")
    .await?;
computer
    .download("/tmp/screen.mp4", "screen.mp4")
    .await?;
```

The duration-based Rust helper uses X11 capture. Use the profile-aware CLI recording commands for X11 or Wayland.

CLI:

```bash
BOX=$(computer new --video)
computer record "$BOX" start --fps 20
computer record "$BOX" status
computer record "$BOX" stop screen.mp4
```

Add `Extras::audio()` when the recording also needs sound.

The [recording example](examples/recording.rs) shows how to build an animated GIF from selected screenshots.

## Human control

### Hand over exclusive control

`hand_over()` gives a person exclusive input through a browser:

```rust
let takeover = computer.hand_over().await?;
let control_url = takeover.url();

let frame = computer.screenshot().await?;
assert!(
    computer
        .click(Point::new(640, 400), Button::Left)
        .await
        .is_err()
);

takeover.end().await?;
```

CLI:

```bash
computer takeover "$BOX"
computer release "$BOX"
```

The program can continue to read the screen while the person controls it. Its input operations return an error.

Use `share()` only when the person and the program must send input at the same time:

```rust
let shared = computer.share().await?;
```

Shared input can race. Use exclusive handover when both sides do not need simultaneous control.

### Wait for the person to leave

```rust
let takeover = computer.hand_over().await?;

computer
    .wait_until_free(Duration::from_secs(600))
    .await?;
takeover.end().await?;

let new_frame = computer.screenshot().await?;
```

Always take a new screenshot after a handover.

### Reclaim a desktop

An attached process can end a takeover that outlived its owner:

```rust
let computer = Computer::attach("my-box").await?;

if computer.person_driving().await {
    computer.reclaim().await?;
}
```

CLI:

```bash
computer release "$BOX"
```

The image also refuses raw synthetic input from inside the box while a person has exclusive control.

## Configure a desktop

### Create a box with `computer new`

`computer new` converts command flags into a portable `Spec` and `Placement`, starts the box, and prints its ID to standard output. Status text and the viewer URL go to standard error, so command substitution receives only the ID:

```bash
BOX=$(computer new --url https://example.com)
computer box "$BOX"
```

Choose the display and capacity:

```bash
BOX=$(computer new \
    --size 1920x1080 \
    --screens 2 \
    --wayland \
    --memory 4g \
    --cpus 2 \
    --runtime podman)
```

Build applications and features into the image:

```bash
BOX=$(computer new \
    --app gimp,vscode \
    --package jq \
    --package ripgrep \
    --wide-fonts \
    --audio \
    --video \
    --dock \
    --accessibility)

WAYLAND_BOX=$(computer new --wayland --x11-apps)
```

- `--wide-fonts` adds CJK and emoji fonts.
- `--audio` adds the sound server.
- `--video` adds recording support.
- `--dock` adds the desktop launcher.
- `--x11-apps` adds Xwayland to a Wayland image.
- `--accessibility` enables native widget operations.

Applications, packages, and features are image inputs. They cannot be added to a running box, and the first box with a new combination must build an image.

Set network and lifetime policy:

```bash
BOX=$(computer new --no-network --ttl 60 --idle 10)
```

`--ttl` removes the box after a fixed number of minutes. `--idle` removes it after that many minutes without server activity. `--no-network` blocks outbound network access from the desktop.

Complete command reference:

```text
computer new [--size WIDTHxHEIGHT] [--screens N] [--wayland] [--url URL]
             [--app NAME]... [--package PACKAGE]...
             [--wide-fonts] [--audio] [--video] [--dock]
             [--x11-apps] [--accessibility]
             [--no-network] [--memory SIZE] [--cpus N]
             [--runtime NAME] [--ttl MINUTES] [--idle MINUTES]
             [--spec FILE|-]
```

Repeat `--app` and `--package`, or give comma-separated names. `--spec -` reads a create request from standard input. Flags override values from the file. `computer --local new --name NAME` can choose a local runtime name; a server always assigns its own box ID and refuses `--name`.


### Builder options

Use the builder to change the defaults:

```rust
let computer = Computer::builder()
    .size(1920, 1080)
    .network(false)
    .memory("2g")
    .runtime("podman")
    .name("my-box")
    .keep_on_drop(true)
    .launch()
    .await?;
```

CLI:

```bash
BOX=$(computer new --size 1920x1080 --no-network --memory 2g --runtime podman --ttl 60)
```

`computer new` keeps the desktop after the command exits. `--app gimp` and `--package jq` install into the image, and `--spec box.json` takes a file shaped like the body of `POST /v1/boxes` for what has no flag; a flag goes over the file.

- `network(false)` blocks outbound network access from the desktop.
- `runtime()` also accepts `nerdctl`.
- `keep_on_drop(true)` leaves the desktop running when the handle is dropped.
- `expires_after(duration)` removes the desktop after a fixed time.
- `expires_when_idle(duration)` removes it after a period without activity through that handle. Call `touch()` when work reaches the box by another path.

`Computer::builder().config()?` returns the resolved image, ports, environment, and boot command without starting a desktop.

### Use a portable specification

A `Spec` describes the desktop, installed applications, and access policy. A `Placement` describes where it runs and its resource and lifetime limits:

```json
{
  "spec": {
    "desktop": {
      "server": "x11",
      "width": 1280,
      "height": 800,
      "features": ["wide_fonts", "accessibility"]
    },
    "policy": {
      "network": true
    }
  },
  "placement": {
    "runtime": "docker",
    "memory": "2g",
    "expires_after_secs": 3600
  }
}
```

```bash
BOX=$(computer new --spec box.json)
```

The same specification can be placed in a container, microVM, or supported cloud sandbox. Unknown keys are refused instead of ignored. See [examples/box.json](examples/box.json) and [examples/from_spec.rs](examples/from_spec.rs).

### Install and open applications

List the built-in application catalog, install applications into a new image, and open one by name:

```bash
computer apps
BOX=$(computer new --app gimp --app vscode)
computer app "$BOX" gimp
```

`app` waits until the application's window has drawn. Applications and packages are image inputs. They cannot be added to a running box.

### Add fonts and packages

The default image includes Latin fonts. Add wider language support or Debian packages when required:

```rust
let desktop_with_wide_fonts = Computer::builder()
    .wide_fonts()
    .launch()
    .await?;

let desktop_with_packages = Computer::builder()
    .packages(["vim", "curl"])
    .launch()
    .await?;
```

CLI:

```bash
BOX=$(computer new --wide-fonts --accessibility --video)
```

`Extras::audio()`, `Extras::video()`, `Extras::accessibility()`, and `Extras::everything()` provide common package sets.

Each builder package helper sets the complete extra package list. Use one combined list when you need custom packages and a preset together. The package list is part of the image tag. The first launch with a new list builds a new image.

### Keep browser data

Keep the complete Chromium profile in a named container volume:

```rust
let computer = Computer::builder()
    .profiles("agent-work")
    .launch()
    .await?;
```

The profile includes logins, history, extensions, and browser storage. It stays on the host and can be used by only one desktop at a time.

Use session export when the data must move between hosts. Use a named profile when all browser state must stay on one host.

### Attach to a running desktop

```rust
let computer = Computer::attach("my-box").await?;
```

CLI:

```bash
computer ls
computer screenshot "$BOX" screen.png
```

CLI commands attach to the desktop named by their `<box>` argument.

The attached desktop keeps its windows, browser profile, and files. Dropping an attached handle does not remove a desktop that the handle did not create.

## Box lifecycle and history

### List and inspect boxes

List all boxes known to the selected server:

```bash
computer ls
```

Each row contains the box ID, screen size, screen count, and a non-ready state when applicable. Inspect one box in detail:

```bash
computer box "$BOX"
```

`computer box` reports:

- ID and state: `Ready`, `Paused`, or `Stopped`
- configured screen count and size
- the digest of the portable specification
- creation and expiry times
- viewer and direct DevTools URLs when available

The box ID is the value accepted by every command that takes `<box>`. Treat viewer and DevTools URLs as credentials when they contain access tokens.

`computer box` uses the server API and is not available with `--local`. Local `computer ls` asks the default Docker runtime what is still running.

### Pause, stop, resume, and remove

```bash
computer pause "$BOX"
computer resume "$BOX"

computer stop "$BOX"
computer resume "$BOX"

computer rm "$BOX"
```

A paused box keeps its memory and ports but uses no processor. A stopped box keeps its writable filesystem without keeping its memory. Resuming a stopped box starts a fresh desktop on new viewer ports. Removing a box deletes its files.

The Rust API exposes the same lifecycle:

```rust
computer.pause().await?;
computer.resume().await?;

computer.stop().await?;
let computer = computer.start(Duration::from_secs(90)).await?;
```

Use `--ttl MINUTES` for a fixed lifetime and `--idle MINUTES` for an inactivity limit. `computerd` also sweeps expired boxes that outlive the process that created them.

### Run action batches

A batch holds one screen across several operations, stops at the first refusal by default, and returns one final frame:

```json
[
  { "type": "open_url", "url": "https://example.com/order" },
  {
    "type": "on_page",
    "what": { "op": "fill", "query": "Name", "text": "Ada" }
  },
  {
    "type": "on_page",
    "what": { "op": "click", "query": "Continue" }
  },
  {
    "type": "on_page",
    "what": { "op": "wait_for", "query": "Details" }
  }
]
```

```bash
computer batch "$BOX" actions.json --settle 400
```

Use `--keep-going` only when later steps do not depend on earlier steps. The REST equivalent is `POST /v1/boxes/{id}/screens/{screen}/actions`.

CLI batches, traces, forks, pause, stop, and resume use the server API. The ephemeral server can run a batch, but a long-lived `computerd` is required to retain useful history across commands.

### Trace and fork

`computerd` records actions, frames, commands, lifecycle changes, file transfers, and custody changes:

```bash
computer trace "$BOX"
NEW_BOX=$(computer fork "$BOX")
```

A fork launches the same specification and replays the trace. It reconstructs the work; it does not copy a running machine. Page changes, timing, and unrecorded file or clipboard bytes can make the result differ.

## Server, REST, and MCP

### Run `computerd`

The daemon listens on `127.0.0.1:8080` by default:

```bash
computerd
curl http://127.0.0.1:8080/v1/health
```

Bind outside loopback only with a server token:

```bash
COMPUTER_SERVER_ADDR=0.0.0.0:8080 \
COMPUTER_SERVER_TOKEN="$(openssl rand -hex 32)" \
computerd
```

Clients send the token as `Authorization: Bearer ...`. The gate protects REST and MCP. Viewer links use separate per-box credentials.

The REST API uses shared request and response types from [`computer-api`](crates/computer-api). Box creation, action batches, and forks accept idempotency keys so a transport retry does not repeat a click or create a second box. See the [server guide](crates/computer-server/README.md) for routes and semantics.

On restart, `computerd` adopts boxes that it left on the configured runtimes. Docker is checked by default. Set `COMPUTER_SERVER_RUNTIMES=docker,podman` to check more container runtimes, and use `COMPUTER_SERVER_SANDBOXES=e2b` when the daemon was built with E2B support.

### REST quick start

Create a box on a loopback server:

```bash
BASE=http://127.0.0.1:8080

BOX=$(
  curl -fsS "$BASE/v1/boxes" \
    -H 'content-type: application/json' \
    -H 'idempotency-key: create-demo-1' \
    -d '{
      "spec": {
        "desktop": { "width": 1280, "height": 800 },
        "policy": { "network": true }
      },
      "placement": { "expires_after_secs": 3600 }
    }' |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'
)
```

Drive it with one action batch and one final frame:

```bash
curl -fsS "$BASE/v1/boxes/$BOX/screens/0/actions" \
  -H 'content-type: application/json' \
  -H 'idempotency-key: open-demo-1' \
  -d '{
    "actions": [
      { "type": "open_url", "url": "https://example.com" },
      { "type": "wait_still", "settle_ms": 400, "within_ms": 10000 }
    ],
    "want": ["frame", "cursor"]
  }'
```

Inspect and remove it:

```bash
curl -fsS "$BASE/v1/boxes/$BOX"
curl -fsS -X DELETE "$BASE/v1/boxes/$BOX" \
  -H 'x-computer-confirm-delete: true'
```

Add `Authorization: Bearer <token>` to every request when `COMPUTER_SERVER_TOKEN` is set. API errors have one shape: `code`, `message`, and `retryable`.

### Persist server state

The default in-memory store keeps traces only for the life of `computerd`. Select a durable backend when traces and frames must survive a restart:

```bash
COMPUTER_STORAGE_BACKEND=local \
COMPUTER_STATE_DIR=/var/lib/computer \
computerd
```

SQLite, PostgreSQL, and S3 are optional build features:

```bash
cargo install --path . --locked --features sqlite,postgres,s3
```

SQL backends use `COMPUTER_STATE_URL`. S3 uses `COMPUTER_S3_ENDPOINT`, `COMPUTER_S3_BUCKET`, credentials from the environment, and optional region and prefix settings.

By default, old frames are retained for two hours and trace entries for seven days. `COMPUTER_KEEP_FRAMES_SECS`, `COMPUTER_KEEP_ENTRIES_SECS`, and `COMPUTER_PRUNE_SECS` change those windows.

### Configure MCP

For an MCP host that launches a local process:

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

Add `COMPUTER_SERVER_TOKEN` to the stdio environment when the daemon is gated.

For a remote host:

```text
URL:           https://boxes.example.com/mcp
Transport:     Streamable HTTP
Authorization: Bearer <COMPUTER_SERVER_TOKEN>
```

Put TLS in front of `computerd` and forward WebSocket upgrades when an MCP Apps host must render the live screen. `COMPUTER_PUBLIC_URL` sets the public origin when a reverse proxy hides it.

An MCP Apps host can render `ui://computer/screen.html` beside `launch_box`, `open_screen`, and `hand_over`. The page receives a short-lived screen ticket under `_meta`; the model does not receive it.

See the [MCP guide](crates/computer-mcp/README.md).

## Runtimes

The desktop operations stay the same across runtimes. Startup, isolation, image storage, networking, and cleanup can differ.

### Containers

Docker is the default. Podman and nerdctl use the same image and desktop API:

```rust
let computer = Computer::builder()
    .runtime("podman")
    .launch()
    .await?;
```

A container shares the host kernel. The runtime selects free host ports, and the desktop is removed when its owning handle is dropped unless configured otherwise.

### microVMs

A microVM boots its own kernel and gives a stronger isolation boundary. It starts more slowly and uses a separate image store.

The included integration targets microsandbox 0.6. Import the image into the hypervisor before the first launch. See [examples/microvm.rs](examples/microvm.rs) for the complete flow.

Other hypervisors can implement `MicroVmApi`.

### Cloud sandboxes

The included E2B integration runs the desktop away from the local host. Build with the E2B HTTP client and set its API key:

```toml
computer = { git = "https://github.com/CITGuru/computer", default-features = false, features = ["e2b"] }
```

```bash
export E2B_API_KEY=...
```

E2B uses templates instead of local container images. Its builder accepts only part of Dockerfile syntax, and its process runs as uid 1000. Generate a compatible build context from the bundled X11 image:

```bash
python3 crates/computer-core/images/context.py \
  crates/computer-core/images/desktop \
  /tmp/e2b-ctx \
  --for e2b

e2b template create computer-desktop \
  -p /tmp/e2b-ctx \
  -d Dockerfile \
  -c "/usr/local/bin/computer-desktop" \
  --ready-cmd "true" \
  --cpu-count 2 \
  --memory-mb 2048
```

The transform removes instructions that E2B rejects or overrides and makes the desktop home writable by uid 1000. Pass the template ID printed by E2B to the example:

```bash
cargo run --features e2b --example e2b -- <template-id>
```

Add `--keep` to leave the sandbox running until its deadline. The takeover example gives the public control viewer to a person:

```bash
cargo run --features e2b --example e2b_takeover -- <template-id> "search text"
```

The viewer URL is withheld by default. A machine configured with `public_viewer(true)` is internet-reachable and must use `Auth::Password` or `Auth::Token`; launch is refused without that gate.

Remote cloud profiles do not expose Chrome DevTools. Screen input, screenshots, clipboard operations, human control, and file transfer remain available.

### X11 and Wayland

X11 is the default display profile. Select the Wayland profile explicitly:

```rust
let computer = Computer::builder()
    .profile(Arc::new(WaylandProfile))
    .launch()
    .await?;
```

CLI:

```bash
BOX=$(computer new --wayland)
```

The public desktop API is the same for both profiles.

The X11 image uses Xvfb, fluxbox, x11vnc, ImageMagick, and `xdotool`. The Wayland image uses headless sway, wayvnc, `grim`, and one virtual pointer and keyboard per screen that stay for the life of the screen, so a button or a modifier can be held between two steps as on X11.

Wayland cannot read the global pointer after a person moves it. `cursor()` returns `Unsupported` after a handover until the driver moves the pointer again.

## Custom desktops and runtimes

### Use a custom image

Build a local Docker context:

```rust
let computer = Computer::builder()
    .image_dir("images/my-desktop")
    .launch()
    .await?;
```

Or pull an existing image:

```rust
let computer = Computer::builder()
    .image("registry.example.com/desktop:1")
    .launch()
    .await?;
```

A local context must contain a `Dockerfile` that implements the selected profile. A registry image cannot be combined with extra packages because the crate does not build that image.

An image can declare its contract:

```dockerfile
LABEL computer.profile="computer-desktop"
```

The crate refuses a declared profile mismatch before startup.

See [examples/custom_image.rs](examples/custom_image.rs) and [examples/images/acme/](examples/images/acme/) for a small derived image.

### Define a desktop profile

Use `ProfileBuilder` when an image keeps most of a shipped contract:

```rust
let profile = ProfileBuilder::new(X11Profile)
    .name("my-desktop")
    .image_dir("images/mine")
    .screen_commands(CommandScreen::new("my-screen"))
    .wallpaper_runtime(CommandWallpaperRuntime::new("my-wallpaper"))
    .build();

let computer = Computer::builder()
    .profile(Arc::new(profile))
    .launch()
    .await?;
```

A profile defines the image contract, ports, geometry, environment, capabilities, and display driver. A new display server implements `Desktop` and `DesktopFactory`.

### Add a cloud sandbox vendor

`RemoteApi` is the common interface for E2B, Daytona, Modal, and similar services. An adapter creates, finds, and removes a sandbox; runs commands; and reads and writes files.

`RemoteMachine` supplies shared lifetime, naming, and keep-alive behavior. `RemoteProfile` maps vendor endpoints and removes capabilities that the remote service cannot expose.

Run the local reference adapter before you connect an external API:

```bash
cargo run --example custom_sandbox
```

See [examples/custom_sandbox.rs](examples/custom_sandbox.rs) for a complete adapter and `computer::testing::ScriptedRemote` for tests without an account or network.

### Audit a desktop

`DesktopSupport` states what a profile provides. `audit` tests those claims against a running desktop:

```rust
let audit = computer::audit(&computer).await;
assert!(audit.ok());
```

The audit checks the screen, pointer, DevTools connection, clipboard, viewer, and handover where the profile claims support.

### Clean up expired desktops

```rust
let removed = computer::sweep_expired(
    &DockerMachine::default(),
    SystemTime::now(),
)
.await?;
```

CLI:

```bash
computer sweep
```

`computer sweep` checks the local default Docker runtime, even when a remote server is configured. `computerd` reaps fleet boxes on its own cadence. Use deadlines for services that can stop before normal shutdown.

## What is inside the box

The default image is based on `debian:bookworm-slim`. It includes:

- Xvfb and fluxbox for the default virtual desktop
- Chromium with a separate profile for each screen
- x11vnc, websockify, and noVNC for browser viewing
- `xdotool` for pointer and keyboard input
- ImageMagick for PNG capture
- `xclip` for clipboard and primary selections
- `socat` for the Chrome DevTools bridge
- an input guard that blocks synthetic input during human control

The image tag contains a hash of its source files. A source change creates a new tag instead of reusing stale image contents.

## Security

The local viewer is open by default and is published only on loopback. Anyone who can reach a control port can drive the desktop.

`computerd` also refuses a non-loopback bind without a server token of at least 16 characters. `/v1/health` remains open for health checks.

Publishing outside loopback requires authentication:

```rust
let computer = Computer::builder()
    .auth(Auth::Token)
    .publish_on(Bind::Any)
    .advertise("boxes.example.com")
    .launch()
    .await?;
```

`Auth::Password` prompts in the browser and keeps the password out of the URL. `Auth::Token` puts a ticket in the URL so one link carries access.

Watch and control viewers use separate credentials. Changing the port on a watch URL does not create control access.

The box's raw Chrome DevTools port stays on loopback because CDP has no authentication. Use `computer cdp` to mint a short-lived address through the authenticated server. Treat that address as a credential.

`network(false)` blocks network access from the desktop. It does not protect the viewer.

A control port exists only during human handover. While it is open, the box rejects other synthetic input.

Browser sessions and persistent profiles contain authenticated account state. Treat them as credentials.

## Examples

Start with these:

- [quickstart](examples/quickstart.rs) launches, controls, and captures a desktop.
- [browser](examples/browser.rs) controls Chromium through DevTools.
- [elements](examples/elements.rs) finds and acts on page elements.
- [capture](examples/capture.rs) captures regions and windows at different sizes.
- [takeover](examples/takeover.rs) gives control to a person and takes it back.
- [tour](examples/tour.rs) controls two screens.

The repository also includes:

- [serve](examples/serve.rs) leaves a named desktop running.
- [attach](examples/attach.rs) connects to a running desktop.
- [recording](examples/recording.rs) creates an animated GIF.
- [waiting](examples/waiting.rs) waits for browser state.
- [research](examples/research.rs) searches and reads web pages.
- [from_spec](examples/from_spec.rs) launches from a portable specification.
- [demo](examples/demo.rs) records a multi-step desktop animation.
- [live_desktop](examples/live_desktop.rs) audits the container image.
- [custom_image](examples/custom_image.rs) builds and uses another image.
- [microvm](examples/microvm.rs) runs the desktop with microsandbox.
- [custom_sandbox](examples/custom_sandbox.rs) implements a sandbox vendor.
- [e2b](examples/e2b.rs) runs in an E2B sandbox.
- [e2b_takeover](examples/e2b_takeover.rs) gives an E2B desktop to a person.
- [client drive](crates/computer-client/examples/drive.rs) exercises the REST client end to end.

Run an example with Cargo:

```bash
cargo run --example quickstart
cargo run -p computer-client --example drive
```

## Workspace crates

- [`computer`](Cargo.toml) is the root package. It re-exports the Rust desktop API and, by default, builds `computer` and `computerd`.
- [`computer-core`](crates/computer-core) implements boxes, desktops, profiles, display drivers, images, and runtime adapters.
- [`computer-types`](crates/computer-types) holds portable specifications and shared values.
- [`computer-api`](crates/computer-api) defines REST wire types.
- [`computer-client`](crates/computer-client) is the Rust REST client.
- [`computer-server`](crates/computer-server) implements `computerd`.
- [`computer-cli`](crates/computer-cli) implements the commands.
- [`computer-mcp`](crates/computer-mcp) maps MCP tools and the live screen app to REST.
- [`computer-storage`](crates/computer-storage) provides memory, local, SQLite, PostgreSQL, and S3 storage.

## Development and testing

Run the same static checks as CI:

```bash
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo deny check
```

Also check optional backends and code that is not compiled by the normal workspace commands:

```bash
cargo clippy -p computer --no-default-features -- -D warnings
cargo test -p computer-storage --features sqlite,postgres,s3
cargo test -p computer-server --features sqlite,s3
cargo clippy -p computer-core --features microsandbox --all-targets -- -D warnings
python3 scripts/check-page-scripts.py
crates/computer-mcp/ui/build.sh
git diff --exit-code -- crates/computer-mcp/ui/screen.html
for script in crates/computer-core/images/*/*.sh; do bash -n "$script"; done
```

The normal test suite does not need a container runtime. Live tests are ignored by default:

```bash
cargo test --test live -- --ignored --nocapture
cargo test --test live_apps -- --ignored --nocapture
cargo test --test live_auth -- --ignored --nocapture
cargo test --test live_microvm -- --ignored --nocapture
cargo test --test live_extras -- --ignored --nocapture
cargo test --test live_session -- --ignored --nocapture
cargo test --test live_wayland -- --ignored --nocapture
cargo test --features e2b --test live_e2b -- --ignored --nocapture
```

`computer::testing` provides test doubles for screens, container runtimes, hypervisors, and remote sandboxes.

## License

MIT. See [LICENSE](LICENSE).