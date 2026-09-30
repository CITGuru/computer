# Drive native windows by widget

This guide drives native applications, such as file dialogs, settings panels, and installers, by the names of their widgets. For when to use this mode and how it compares with the others, see [Control modes](../concepts/control-modes.md#accessibility).

## 1. Accessibility is on by default

Every box has accessibility unless it is launched without it. It starts the AT-SPI bus before the desktop, so applications publish their widgets.

| Interface | Default | To leave it out |
| --- | --- | --- |
| CLI | `computer new` | `--no-accessibility` |
| MCP | `launch_box` | `accessibility: false` |
| REST | No `features` in `spec.desktop` | A `features` list without `accessibility` |
| Rust | `Computer::builder()` | `without(Feature::Accessibility)` |

You cannot enable it on a running box. An application joins the tree only if the bus was running before the application started. On a box with no accessibility, the widget tools fail with a message that says so.

Accessibility works on the X11 and Wayland images.

## 2. Read the tree

```bash
computer widget "$BOX" tree --depth 4
computer widget "$BOX" tree --app gimp --depth 6
```

Each node has:

| Field | Meaning |
| --- | --- |
| `app` | The application that owns the widget. |
| `role` | The toolkit's role, such as `push button`, `text`, or `check box`. |
| `name` | The widget's own name. Often empty for a form field. |
| `labelled` | The label that matched the query, when the match came from a label next to the widget. |
| `actions` | The actions that `press` can run. |
| `states` | AT-SPI states, such as `focused`, `enabled`, or `checked`. |
| `at`, `width`, `height` | The center point and size, in screen pixels. Absent when the widget is not on the screen. |
| `value` | The text of the widget, when it has text. |

A tree read stops at depth 4 unless you give `--depth`. A full tree can have thousands of nodes, so narrow it with `--app`.

## 3. Find a widget

```bash
computer widget "$BOX" find "Street"
computer widget "$BOX" find "Street" --role text
computer widget "$BOX" find "OK" --role "push button" --exact
```

A query matches:

- The widget's own name.
- A label next to the widget. A form field usually has no name of its own, so the search also finds the nearest actionable widget to the right of a label with that text on the same row, or below it in the same column, within 220 pixels.

The best match is first. An exact match ranks above a partial match, and a field ranks above the label that names it. Applications with an active window are searched first, so when two windows each have an `OK` button, the active one wins.

| Option | Effect |
| --- | --- |
| `--role R` | Only widgets with this role. Use the role that `find` or `tree` reports. |
| `--exact` | Match all of the text, not part of it. |
| `--app NAME` | Only this application. |
| `--limit N` | Maximum matches. Default 20. |

## 4. Fill a field

```bash
computer widget "$BOX" fill "Street" "12 Bishop Street" --role text
```

`fill` writes the value into the widget's text directly. It does not send key presses. If an application acts on each key press, focus the widget and type instead:

```bash
computer widget "$BOX" focus "Street" --role text
computer keyboard "$BOX" type "12 Bishop Street"
```

A widget that is not editable refuses `fill`, and the error names its role.

## 5. Press a button

```bash
computer widget "$BOX" press "OK" --role "push button"
```

`press` runs the widget's first action. Give `--action` to run a different one:

```bash
computer widget "$BOX" find "Remember me"
computer widget "$BOX" press "Remember me" --action NAME
```

Use a name from the `actions` list that `find` returns.

Toolkits name actions differently. GTK calls the main action of a button `click`, and Qt calls it `Press`. `find` lists the actions of each widget, and the `--action` match ignores case.

## 6. Choose between press and a real click

`press` sends no pointer event. This has two effects:

- It works on a widget that is covered by another window or scrolled out of view.
- An application that watches the pointer, such as for hover effects or drag and drop, sees nothing.

When the application must see a real click, use the widget's `at` point with a screen click:

```bash
computer widget "$BOX" find "Upload" --role "push button"
computer mouse "$BOX" click 640 412 left
```

A widget with no actions cannot be pressed. The error says so and tells you to click its `at` point.

## The same steps in other interfaces

### MCP

The `widget` tool takes `op` with `find`, `tree`, `press`, `fill`, or `focus`:

```json
{ "box_id": "box_…", "op": "fill", "query": "Street", "role": "text", "value": "12 Bishop Street" }
```

See [`widget`](../reference/mcp-tools.md#widget).

### REST

`POST /v1/boxes/{id}/screens/{screen}/desktop/node`:

```json
{ "op": "find", "node": { "query": "Street", "role": "text" }, "limit": 10 }
{ "op": "set", "node": { "query": "Street", "role": "text" }, "value": "12 Bishop Street" }
{ "op": "invoke", "node": { "query": "OK", "exact": true }, "action": "click" }
```

The response has `nodes` for `tree` and `find`, and `node` for the other operations. In a batch, use the `on_node` action with the same body under `what`. See [Native widgets](../reference/rest-api.md#native-widgets).

### Rust

```rust
let computer = Computer::builder().launch().await?;
let screen = computer.primary();

let street = NodeQuery {
    query: "Street".to_string(),
    role: Some("text".to_string()),
    ..NodeQuery::default()
};
screen.set_node(&street, "12 Bishop Street").await?;

let ok = NodeQuery {
    query: "OK".to_string(),
    exact: true,
    ..NodeQuery::default()
};
screen.invoke_node(&ok, None).await?;

if let Some(at) = screen.find_nodes(&street, Some(1)).await?.first().and_then(|node| node.at) {
    screen.click(at, Button::Left).await?;
}
```

| CLI | Rust |
| --- | --- |
| `widget tree` | `nodes(app, depth)` |
| `widget find` | `find_nodes(&query, limit)` |
| `widget focus` | `focus_node(&query)` |
| `widget fill` | `set_node(&query, value)` |
| `widget press` | `invoke_node(&query, action)` |

## During a takeover

While a person has exclusive control, `focus`, `fill`, and `press` are refused. `tree` and `find` continue to work. See [Human control](../concepts/human-control.md).

## Troubleshooting

| Problem | Cause and fix |
| --- | --- |
| "this box has no accessibility tree" | The box was launched without accessibility. Launch a new box with it. |
| The tree is empty, or an application is missing | The application does not publish widgets, or it started before the bus. Close it and open it again. |
| "nothing in the tree matched" | Read the tree with `--app` to see the names the application uses. Try a shorter query, or remove `--role`. |
| The wrong widget matched | Add `--role`, `--exact`, or `--app`. Bring the correct window to the front, because the active application is searched first. |
| A widget has no `at` | It is not on the screen, such as a menu item that was never drawn. Open the menu first, or use `press`. |
| "publishes no action" | Click the widget's `at` point with the mouse. |
| A web page has no useful widgets | Use the browser mode for web pages. See [Control modes](../concepts/control-modes.md#browser). |
