# Drive the browser

This guide does common browser tasks: open pages, find and act on controls, wait for results, read and capture pages, and move a login between boxes. For why and when to use the browser mode, see [Control modes](../concepts/control-modes.md#browser).

On remote runtimes such as E2B and Vercel, page tools are slower. On E2B, they also need a template built from the current image, and they stop working on boxes that a restarted server finds again. See [Browser mode on remote runtimes](../concepts/control-modes.md#browser-mode-on-remote-runtimes).

## The interfaces

| Interface | Where the browser commands are |
| --- | --- |
| CLI | `computer open` and `computer browser <box> …` |
| MCP | `open_url`, `snapshot`, `click_element`, and the other page tools |
| Rust | `computer.browser()` returns a `Devtools`, and `open_page` returns a `Page` |
| REST | `/v1/boxes/{id}/page/…` and the `on_page` action |

For all flags and parameters, see the [CLI reference](../reference/cli.md#browser), the [MCP tools reference](../reference/mcp-tools.md#pages), and the [REST API reference](../reference/rest-api.md#pages).

## Open pages and tabs

```bash
TAB=$(computer open "$BOX" https://example.com)
computer open "$BOX" https://example.org --target current
computer open "$BOX" https://mail.example.com --label mail
```

`open` opens a new tab, brings it to the front, and prints the tab ID. `--target current` navigates the tab at the front instead. `--label` gives the tab a name. Every `--tab` option then accepts the name as well as the ID.

```bash
computer browser "$BOX" tabs
computer browser "$BOX" switch mail
computer browser "$BOX" close "$TAB"
computer browser "$BOX" back
```

Give `--tab` to each command when more than one tab is open. Without it, the command acts on the tab at the front, and that can change.

MCP: `open_url` (with `label`), `tabs`, and `history`.

```rust
let browser = computer.browser().ok_or("no DevTools port")?;
let mut page = browser
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;
page.navigate("https://example.org").await?;
```

Use `open_page`, not `open` and then a load wait. A new tab shows `about:blank` first, and that page is already loaded.

## See what the page has

A snapshot lists the controls on the page in document order. Each control has a reference, such as `@e12`, that you can use as a query.

```bash
computer browser "$BOX" snapshot --urls --quiet 400
computer browser "$BOX" click @e4
computer browser "$BOX" snapshot --delta
```

| Option | Effect |
| --- | --- |
| `--urls` | Add the address of each link. |
| `--delta` | Show only the controls that appeared, changed, or went away since the last snapshot. |
| `--quiet MS` | Wait until the page does not change for this time before the list is made. |
| `--scope QUERY` | List only the controls inside one element, such as a form. |

A reference stays with its element while the page stays loaded. A navigation clears all references. After a page has a snapshot, each action also reports the controls that it made appear, change, or go away, so you often do not need a second snapshot.

`find` searches for elements and gives their role, state, and position:

```bash
computer browser "$BOX" find "Submit" --role button
computer browser "$BOX" find --role textbox --limit 20
```

Each match ends with a selector that names exactly one element. Use that selector in the next command.

```rust
let taken = page.snapshot(None, None).await?;
page.click_on("@e4", Button::Left).await?;
let changed = page.snapshot_delta(None, None).await?;
let fields = page.find("input", Some(10), None, None).await?;
```

A query also reaches into frames from the same origin as the page. Frames from other origins are not included.

## Act on controls

```bash
computer browser "$BOX" fill "Email" "agent@example.com"
computer browser "$BOX" select "Country" "United Kingdom"
computer browser "$BOX" check "I agree"
computer browser "$BOX" upload "Attachment" ./report.pdf
computer browser "$BOX" click "Submit"
```

| Control | Command | MCP |
| --- | --- | --- |
| Text field, date, color, slider | `fill` | `fill_field` |
| Checkbox, radio | `check`, `uncheck` | `check` |
| Dropdown | `options`, `select`, `deselect` | `dropdown` |
| File input | `upload` | `upload_file` |
| Button, link | `click` | `click_element` |
| Keyboard focus, no click | `focus` | `focus` |
| Pointer over, no click | `hover` | `hover` |
| Drag one element to another | `drag` | `drag_element` |

`upload` reads files from the host and copies them into the box first. Add `--in-box` when the paths are already in the box. The MCP `upload_file` tool takes paths in the box, so put the file there with `write_file` first.

`click --new-tab` opens a link in a new tab, brings that tab to the front, and names it.

`click`, `hover`, and `drag` accept `--smooth` or `--human` to move the pointer along a path, for pages that watch pointer movement. `--seed` repeats the same path.

```rust
page.fill("Email", "agent@example.com").await?;
page.choose("Country", &["United Kingdom".to_string()], false).await?;
page.check("I agree", true).await?;
page.upload("Attachment", &["/tmp/report.pdf".to_string()]).await?;
page.click_on("Submit", Button::Left).await?;
```

## Wait for a result

After an action that loads a page or fetches data, wait for what you expect. Do not use a fixed delay.

```bash
computer browser "$BOX" wait "Order confirmed" --within 10000
computer browser "$BOX" wait ".spinner" --gone
computer browser "$BOX" wait "Pay now" --enabled
computer browser "$BOX" wait --load
computer browser "$BOX" wait --quiet 500
computer browser "$BOX" wait "Success" --or "Payment failed,Try again"
computer browser "$BOX" wait --fn "location.pathname === '/done'"
```

| Option | Waits for |
| --- | --- |
| `<query>` | The element to appear. |
| `--gone` | The element to go away. |
| `--enabled` | The element to accept input. A disabled button matches a query, so use this before you click it. |
| `--load` | The document to load. |
| `--quiet MS` | The page to stop changing for this time. |
| `--or TEXT,TEXT` | Any of these texts. The result says which one matched. |
| `--fn JS` | A JavaScript expression to be truthy. An exception counts as "not yet". |
| `--within MS` | The time limit. |

MCP: `wait_for`, with `gone`, `enabled`, `load`, `quiet_ms`, `or`, `until`, and `within_ms`.

```rust
page.wait_for("Order confirmed", false, Duration::from_secs(10)).await?;
page.quiet(Duration::from_millis(500), Duration::from_secs(10)).await?;
page.wait_until_true("location.pathname === '/done'", Duration::from_secs(10)).await?;
```

## Read a page

```bash
computer browser "$BOX" read --limit 4000
computer browser "$BOX" read --format raw
```

`read` returns the page as Markdown (default), plain text, or raw HTML. It includes text below the visible area and the address behind each link. Use `read` to learn what a page says, and a screenshot to learn where something is.

MCP: `read_page`.

```rust
let text = page.read(Reading::Markdown, Some(4_000), Some(20)).await?;
println!("{}\n{}", text.title, text.text);
```

## Capture a page

```bash
computer browser "$BOX" screenshot page.png
computer browser "$BOX" screenshot full.jpg --full --format jpeg --quality 70
computer browser "$BOX" screenshot labeled.png --annotate
computer browser "$BOX" pdf page.pdf --landscape
```

A page screenshot shows only the page: no window frame, address bar, or pointer. `--full` captures the full scrollable page, as JPEG unless you set `--format`. `--annotate` draws each control's reference from the last snapshot on the image.

`pdf` prints the page with text as text. MCP: `page_screenshot` and `page_pdf`. The MCP `page_pdf` tool writes the file in the box.

## Run JavaScript

```bash
computer browser "$BOX" eval "document.title"
computer browser "$BOX" eval "Array.from(document.links).map(a => a.href)"
```

`await` works. Return plain values: a DOM node returns `{}`. MCP: `evaluate`. Rust: `page.evaluate(js)`.

`Page::call` in Rust sends any CDP command that the crate does not wrap.

## Console and errors

```bash
computer browser "$BOX" console --limit 50
computer browser "$BOX" errors --clear
```

The console log has the page's console calls, uncaught errors, and problems that the browser reported, such as failed requests. `errors` shows only failures. `--clear` empties the log after it is read, so the next read shows only new lines. MCP: `console`.

Read the console when a click does nothing, or when a page shows an error with no explanation.

## Dialogs

While a page has a dialog open, no page command works.

- An alert is accepted automatically. The command that opened it gives its text.
- A confirm, a prompt, or a leave-page dialog makes the command that opened it fail at once, with the dialog text.

Answer it:

```bash
computer browser "$BOX" dialog accept
computer browser "$BOX" dialog accept "text for a prompt"
computer browser "$BOX" dialog dismiss
```

MCP: `dialog`.

## Keep the page and the screen aligned

The screen shows only the tab at the front. Before you mix page commands with screen coordinates, bring the correct tab to the front:

```bash
computer browser "$BOX" switch "$TAB"
computer screenshot "$BOX" screen.png --tab "$TAB"
```

```rust
page.bring_to_front().await?;
assert!(page.visible().await?);
let front = browser.visible_page().await?;
```

## Separate browser sessions

A browser group is a Chromium browser context. Groups share one Chromium process, but each group has its own cookies, local storage, IndexedDB, and service workers. Use groups to run two logins in one box.

Groups are available only in the Rust library.

```rust
let group = browser.create_group().await?;
let mut other = group
    .open_page("https://example.com", Duration::from_secs(20))
    .await?;
other.evaluate("localStorage.setItem('agent', 'two')").await?;
group.close().await?;
```

A group does not make a new screen. Only one page is at the front, so call `bring_to_front()` before you use screen coordinates.

## Move a login between boxes

Save the cookies and storage of some origins, then load them into another box:

```bash
computer browser "$BOX" state save login.json --origin https://mail.example.com
computer browser "$OTHER" state load login.json
```

Keep the state on the server, not in a file:

```bash
computer browser "$BOX" state save --name work
computer browser "$OTHER" state load --name work
computer browser state list
computer browser state rm work
```

| Option | Effect |
| --- | --- |
| `--origin URL` | An origin to save. With none, the origins of the open tabs. |
| `--session-storage` | Also save session storage. |
| `--indexed-db` | Also save IndexedDB, where some sites keep their login. |
| `--no-local-storage` | Do not save local storage. |

A state file contains live logins. The CLI writes it with mode `0600`. A named state stays on the server until the server restarts.

The MCP tools `save_state` and `load_state` use names only, so the login does not go through the model.

```rust
let origins = ["https://mail.example.com".to_string()];
let session = browser.export_session(&origins, Carry::default()).await?;
let tabs = other_browser.import_session(&session).await?;
```

`Carry::default()` includes cookies and local storage.

To keep the full browser profile on one host, use a profile instead. See [Browser data](configure-a-box.md#browser-data).

### Cookies

```bash
computer browser "$BOX" cookies --url https://example.com
computer browser "$BOX" cookies set theme=dark --url https://example.com
computer browser "$BOX" cookies set --curl "$(pbpaste)"
computer browser "$BOX" cookies clear --url https://example.com
computer browser "$BOX" cookies clear --all
```

`--curl` takes the text from a browser's "Copy as cURL" and sets its cookies. MCP: `cookies`, which shows values only when you set `values: true`.

## Connect Playwright or browser-use

`computer cdp` gives a CDP address through the server, with a short-lived token:

```bash
export CDP=$(computer cdp "$BOX")
computer cdp "$BOX" --ws
computer cdp "$BOX" --ttl 10
```

| Library | Use |
| --- | --- |
| Playwright | `chromium.connectOverCDP(process.env.CDP)` |
| browser-use | `cdp_url=CDP` |
| agent-browser | `agent-browser --cdp "$(computer cdp "$BOX" --ws)" snapshot -i` |

The token is valid for one hour, or for `--ttl` minutes, and ends when the box is removed. Treat the address as a credential.

`--direct` gives the box's own DevTools port. That port has no protection, and only the box's host can reach it. Use the address through the server for anything remote. `computer cdp` needs a server, so it does not work with `--local`.

REST: `POST /v1/boxes/{id}/cdp?ttl_secs=…`.

## Mark page text for a model

A page can contain text that tries to give instructions to a model. To make page text easy to separate from tool text, add `--content-boundaries`:

```bash
computer browser "$BOX" read --content-boundaries
```

The command puts the page text between two markers. The markers contain a nonce that the page cannot know. It works on `read`, `snapshot`, `find`, `eval`, `console`, and `errors`.

Set `COMPUTER_CONTENT_BOUNDARIES=1` to do this for every call, and for the MCP tools `read_page`, `snapshot`, `find`, `evaluate`, and `console`. On `computerd`, it applies to `/mcp`.
