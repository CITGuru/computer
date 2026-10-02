# Control modes

There are three ways to control a box. You can use more than one in the same task.

| Mode | Acts on | Use it for | Needs |
| --- | --- | --- | --- |
| Screen coordinates | Pixels on the screen | Any visible application, browser chrome, and system prompts | Nothing |
| Accessibility | Native widgets, by role and name | File dialogs, settings panels, installers, and other native forms | A box with `accessibility` |
| Browser | Page elements, through Chrome DevTools | Web pages | Nothing on host runtimes. On E2B, a template built from the current image. On Vercel, nothing. |

## Select a mode

1. **Is it a web page?** Use the browser mode.
2. **Is it a native window that publishes its widgets?** Use the accessibility mode.
3. **All other cases:** use screen coordinates.

Browser chrome (the address bar and tabs), permission prompts, and native file choosers are outside the page. Use screen coordinates for them, or a dedicated tool: `upload_file` fills a file input with no chooser, and `dialog` answers alerts and prompts.

## Screen coordinates

Take a screenshot, find a point, and send mouse or keyboard input to it.

```bash
holm screenshot "$BOX" screen.png
holm mouse "$BOX" click 640 400 left
holm keyboard "$BOX" type "hello"
```

```rust
let png = computer.screenshot().await?;
computer.click((640, 400), Button::Left).await?;
computer.type_text("hello").await?;
```

MCP: `screenshot`, then `click`, `type_text`, `press_key`, `drag`, and `scroll`.

Rules:

- Coordinates are device pixels on one screen. `(0, 0)` is the top-left corner.
- Get the coordinates from the most recent full-size screenshot. A point from an old, cropped, or scaled image goes to the wrong place, and nothing tells you.
- After an action that draws, such as opening a menu, wait until the screen is still (`wait_until_still`) before the next screenshot.
- A screenshot does not show the pointer. Use `cursor` to get its position.

This mode works on all runtimes and with all applications. It is also the easiest to break: a page that moves, a window that opens, or a different screen size changes the correct point.

## Accessibility

Native applications publish a tree of widgets through AT-SPI. Each widget has a role, such as `push button` or `text`, and a name. This mode finds widgets by name, so it does not depend on pixels.

A box has accessibility by default. You cannot add it later to a box launched with `--minimal`.

```bash
BOX=$(holm new)
holm widget "$BOX" find "Street" --role text
holm widget "$BOX" fill "Street" "12 Bishop Street"
holm widget "$BOX" press "OK"
```

```rust
let computer = Computer::builder().launch().await?;
let screen = computer.primary();

let street = NodeQuery {
    query: "Street".to_string(),
    role: Some("text".to_string()),
    ..NodeQuery::default()
};
screen.set_node(&street, "12 Bishop Street").await?;
```

MCP: `launch_box` with `accessibility: true`, then `widget`.

Rules:

- A query matches the widget name and also the label next to it, because a form field usually has no name of its own.
- `press` runs the widget's own action and sends no pointer event. It works on a widget that is covered or out of view.
- If the application must receive a real click, use `find` to get the widget's rectangle, then click with screen coordinates.
- Action names come from the toolkit. GTK calls an action `click`, and Qt calls it `Press`. `find` lists them.
- Some applications and custom widgets publish an incomplete tree. Use screen coordinates for them.

## Browser

The browser mode controls Chromium through the Chrome DevTools Protocol (CDP). It acts on page elements, so it continues to work when the window moves or another window covers it.

```bash
TAB=$(holm open "$BOX" https://www.selenium.dev/selenium/web/web-form.html)
holm browser "$BOX" snapshot --tab "$TAB"
# @e2 textbox "Text input"
# @e8 combobox "Dropdown (select)"
# @e12 checkbox "Default checkbox"
# @e15 button "Submit"

holm browser "$BOX" fill @e2 "agent"
holm browser "$BOX" select @e8 "Two"
holm browser "$BOX" check @e12
holm browser "$BOX" click @e15
holm browser "$BOX" wait "Received!" --within 10000
```

MCP: `open_url`, `snapshot`, then `fill_field`, `dropdown`, `check`, `click_element`, and `wait_for`.

### Queries

A query can be:

- Visible text
- An accessible name
- An element ID
- A placeholder
- A CSS selector
- A reference from `snapshot`, such as `@e12`

Use a reference after a snapshot, because it names one element. A reference stays the same across snapshots of the same page. A navigation clears all references.

`find` returns a selector that names exactly one element. Use that selector in the next action, not the text, which can match more than one element.

Add `exact` when part of the text can match the wrong control.

### Use the correct action

| Control | Action |
| --- | --- |
| Text field, date, color, slider | `fill` |
| Checkbox, radio | `check` / `uncheck` |
| Dropdown | `select`, `deselect`, `options` |
| File input | `upload` |
| Button, link | `click` |

`fill` refuses a dropdown, checkbox, file input, or button, and names the correct action. A native dropdown opens a menu that no screenshot shows and no click can reach, so `select` is the only way to use one.

Field actions follow an HTML label to its control. They do not guess from nearby text. When a match is ambiguous, the action refuses and names the fields near it.

### Wait, do not sleep

After an action that loads a page or fetches data, use `wait` (`wait_for` in MCP), not a fixed delay. A fixed delay is either too short or wastes time on each step.

- Wait for an element to appear, or with `gone`, to go away.
- Give `or` the text that the page shows on failure, such as `sold out`, so the wait stops early.
- Use `quiet_ms` to wait until the page stops changing.

### Page and screen together

`open` opens a new tab and brings it to the front. The screen then shows that tab. Before you mix page actions and screen coordinates, make sure that the correct tab is at the front:

```bash
holm browser "$BOX" switch "$TAB"
holm screenshot "$BOX" --tab "$TAB"
```

### Other CDP libraries

`holm cdp` gives an address for Playwright, browser-use, agent-browser, or another CDP library:

```bash
export CDP=$(holm cdp "$BOX")
agent-browser --cdp "$(holm cdp "$BOX" --ws)" snapshot -i
```

The address goes through the server and contains a short-lived token, valid for one hour or for `--ttl`.

### Browser mode on remote runtimes

On Vercel, page tools and `holm cdp` reach Chromium at the address that Vercel publishes for port 9223, through the same bridge. The server builds the image, so it is current. Daytona and Modal work the same way.

On E2B, page tools and `holm cdp` reach Chromium at the address that E2B publishes for port 9223. A bridge in the box answers only requests that carry a secret made for that box, so the address alone does not open the browser. Build the template from the current image: an older template does not have the bridge, and Chromium refuses the requests.

Each CDP call goes to the vendor and back, so a page tool takes longer than on a host runtime.

When a server takes a box back after a restart, it restarts the bridge in the box with a new secret, so the secret of an earlier server no longer opens the browser. Page tools then work again on the box. Screenshots, input, clipboard, files, commands, and human control continue to work.

## Compare the modes

| | Screen coordinates | Accessibility | Browser |
| --- | --- | --- | --- |
| Works on | Anything visible | Native applications that publish widgets | Web pages |
| Survives layout changes | No | Yes | Yes |
| Reaches covered or scrolled elements | No | Yes | Yes |
| Real pointer events | Yes | No (`press`) | Yes |
| Setup | None | `accessibility` at launch | None; on E2B, a template built from the current image |
